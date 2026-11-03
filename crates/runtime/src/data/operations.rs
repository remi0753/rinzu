use super::*;
use crate::scalar::{ScalarKind, ScalarValue};
use crate::value::ValueType;
use rinzu_abi::data_operation::DataOperation;

impl ManagedData {
    /// Shallow immutable copies inside one Domain. Every referenced child gains
    /// a root before allocating the destination; Worker transport remains deep.
    pub fn operation(
        &mut self,
        heap: &mut DomainHeap,
        op: DataOperation,
        result: usize,
        types: &[usize],
        slots: &[ManagedSlot],
    ) -> Result<ManagedSlot, DataError> {
        self.operation_hashed(heap, op, result, types, slots, |hash| hash)
    }

    fn operation_hashed(
        &mut self,
        heap: &mut DomainHeap,
        op: DataOperation,
        result: usize,
        types: &[usize],
        slots: &[ManagedSlot],
        remap_hash: fn(u64) -> u64,
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        if types.len() != op.arity() || slots.len() != types.len() {
            return Err(DataError::InvalidEncoding);
        }
        for (&ty, &slot) in types.iter().zip(slots) {
            self.slot_field(heap, ty, slot)?;
        }
        if op == Panic {
            let ManagedSlot::Reference(handle) = slots[0] else {
                return Err(DataError::WrongKind);
            };
            return Err(DataError::Panic(self.string(heap, handle)?));
        }
        if matches!(
            op,
            BytesXor
                | BytesFindAny
                | BytesFindNotAny
                | BytesLowerAscii
                | BytesContainsAny
                | BytesAllIn
                | BytesTrim
                | BytesConcatAll
        ) {
            return self.bytes_operation(heap, op, result, types, slots);
        }
        if op as u32 >= DataOperation::BitAnd as u32 {
            return self.numeric_operation(heap, op, result, slots);
        }
        if op as u32 >= DataOperation::StringLength as u32 {
            return self.text_operation(heap, op, result, types, slots);
        }
        if op as u32 >= DataOperation::ErrorCreate as u32 {
            return self.error_operation(heap, op, result, types, slots);
        }
        if op as u32 >= DataOperation::ByteArray as u32 {
            return self.bytes_operation(heap, op, result, types, slots);
        }
        let ManagedSlot::Reference(handle) = slots[0] else {
            return Err(DataError::WrongKind);
        };
        let reference = self.reference(heap, handle, DataKind::Composite(types[0]))?;
        let (variant, fields) = self.composite_fields_iter(heap, reference, types[0])?;
        if variant.is_some() {
            return Err(DataError::WrongKind);
        }
        let layout = self.value_schema()?.ty(types[0])?.clone();
        let count = match &layout {
            ValueType::List(_) => fields.len(),
            ValueType::Map { .. } => fields.len() / 2,
            _ => return Err(DataError::WrongKind),
        };
        if matches!(op, ListLength | MapLength) {
            let count = i64::try_from(count).map_err(|_| DataError::AddressSpaceExhausted)?;
            return Ok(ManagedSlot::Scalar(
                ScalarValue::from_bits(ScalarKind::I64, count as u64).expect("nonnegative length"),
            ));
        }
        if matches!(op, ListGet | MapGet) {
            let field = self.collection_get_field(heap, op, &layout, fields, slots, remap_hash)?;
            let value = field
                .map(|field| self.root_decoded_slot(heap, field))
                .transpose()?;
            let output = self.create_composite(
                heap,
                result,
                Some(u32::from(value.is_some())),
                value.as_slice(),
            );
            // The returned Option traces its child. Retire its temporary alias
            // so repeated reads create no persistent roots for any elements.
            if let Some(ManagedSlot::Reference(handle)) = value {
                self.release(heap, handle)?;
            }
            return output.map(ManagedSlot::Reference);
        }
        // End the readonly loan before allocating. Source containers and input
        // arguments stay rooted; allocation never moves/collects this heap.
        let child_types = super::composite::child_types(&layout, None, fields.len())?;
        let extra = match op {
            ListAppend => 1,
            MapInsert => 2,
            _ => 0,
        };
        let mut children = Vec::with_capacity(fields.len() + extra);
        for (ty, field) in child_types.iter().zip(fields) {
            children.push(self.decode_slot_field(heap, ty, field)?);
        }
        let index = if matches!(op, ListSet | ListRemove) {
            match slots[1] {
                ManagedSlot::Scalar(value) if value.kind() == ScalarKind::I64 => {
                    usize::try_from(value.bits() as i64)
                        .ok()
                        .filter(|i| *i < count)
                }
                _ => return Err(DataError::InvalidEncoding),
            }
        } else if matches!(op, MapInsert | MapRemove) {
            let ValueType::Map { key, .. } = layout else {
                return Err(DataError::WrongKind);
            };
            let schema = self.value_schema()?;
            let wanted = self.snapshot(heap, key, slots[1])?;
            let wanted_hash = remap_hash(schema.key_hash(key, &wanted)?);
            let mut found = None;
            for (index, pair) in children.chunks_exact(2).enumerate() {
                let field = self.decoded_slot_field(heap, key, pair[0])?;
                let candidate = self.snapshot_field(heap, key, field)?;
                if remap_hash(schema.key_hash(key, &candidate)?) == wanted_hash
                    && schema.equal(key, &candidate, &wanted)?
                {
                    found = Some(index);
                    break;
                }
            }
            found
        } else {
            None
        };
        let mut option = None;
        match op {
            ListAppend => children.push(self.decode_rooted_slot(heap, slots[1])?),
            ListSet | ListRemove => {
                if let Some(index) = index {
                    if op == ListSet {
                        children[index] = self.decode_rooted_slot(heap, slots[2])?;
                    } else {
                        children.remove(index);
                    }
                    let handle = self.create_composite_decoded_hashed(
                        heap, types[0], None, &children, remap_hash,
                    )?;
                    option = Some(Some(ManagedSlot::Reference(handle)));
                } else {
                    option = Some(None);
                }
            }
            MapInsert => {
                if let Some(index) = index {
                    children[2 * index + 1] = self.decode_rooted_slot(heap, slots[2])?;
                } else {
                    for &slot in &slots[1..] {
                        children.push(self.decode_rooted_slot(heap, slot)?);
                    }
                }
            }
            MapRemove => {
                if let Some(index) = index {
                    children.drain(2 * index..2 * index + 2);
                }
            }
            MapKeys => {
                let length = children.len() / 2;
                for index in 0..length {
                    children[index] = children[index * 2];
                }
                children.truncate(length);
            }
            ListLength | MapLength | ListGet | MapGet => unreachable!(),
            _ => unreachable!("non-collection operation dispatched above"),
        }
        if let Some(value) = option {
            let output = self.create_composite_hashed(
                heap,
                result,
                Some(u32::from(value.is_some())),
                value.as_slice(),
                remap_hash,
            );
            if let Some(ManagedSlot::Reference(handle)) = value {
                self.release(heap, handle)?;
            }
            return output.map(ManagedSlot::Reference);
        }
        self.create_composite_decoded_hashed(heap, result, None, &children, remap_hash)
            .map(ManagedSlot::Reference)
    }

    fn collection_get_field(
        &self,
        heap: &DomainHeap,
        op: DataOperation,
        layout: &ValueType,
        mut fields: impl ExactSizeIterator<Item = HeapValue> + Clone,
        slots: &[ManagedSlot],
        remap_hash: fn(u64) -> u64,
    ) -> Result<Option<super::composite::DecodedSlot>, DataError> {
        let child_types = super::composite::child_types(layout, None, fields.len())?;
        // Validate every field before lookup, including a malformed tail after
        // a hit/miss. Reading the collection creates no aliases for its siblings.
        for (ty, field) in child_types.iter().zip(fields.clone()) {
            if let super::composite::DecodedSlot::Reference(reference, _) =
                self.decode_slot_field(heap, ty, field)?
            {
                // Preserve root()'s target validation without registering an alias.
                heap.header(reference)?;
            }
        }
        let count = if matches!(layout, ValueType::Map { .. }) {
            fields.len() / 2
        } else {
            fields.len()
        };
        if op == DataOperation::ListGet {
            let ManagedSlot::Scalar(value) = slots[1] else {
                return Err(DataError::InvalidEncoding);
            };
            if value.kind() != ScalarKind::I64 {
                return Err(DataError::InvalidEncoding);
            }
            let Some(index) = usize::try_from(value.bits() as i64)
                .ok()
                .filter(|i| *i < count)
            else {
                return Ok(None);
            };
            let ty = child_types
                .iter()
                .nth(index)
                .ok_or(DataError::InvalidEncoding)?;
            return self
                .decode_slot_field(
                    heap,
                    ty,
                    fields.nth(index).ok_or(DataError::InvalidEncoding)?,
                )
                .map(Some);
        }
        let ValueType::Map { key, value } = layout else {
            return Err(DataError::WrongKind);
        };
        let schema = self.value_schema()?;
        if !schema.is_key(*key)? {
            return Err(crate::value::ValueError::NotKey.into());
        }
        let wanted = self.snapshot(heap, *key, slots[1])?;
        let wanted_hash = remap_hash(schema.key_hash(*key, &wanted)?);
        for _ in 0..count {
            let key_field = fields.next().ok_or(DataError::InvalidEncoding)?;
            let value_field = fields.next().ok_or(DataError::InvalidEncoding)?;
            let candidate = self.snapshot_field(heap, *key, key_field)?;
            if remap_hash(schema.key_hash(*key, &candidate)?) == wanted_hash
                && schema.equal(*key, &candidate, &wanted)?
            {
                return self.decode_slot_field(heap, *value, value_field).map(Some);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{Value, ValueSchema};
    use std::sync::Arc;

    #[test]
    fn collection_updates_only_publish_the_result_and_preserve_sources_through_gc() {
        use DataOperation::*;
        for op in [
            ListAppend, ListSet, ListRemove, MapInsert, MapRemove, MapKeys,
        ] {
            for missing in [false, true] {
                let mut heap = DomainHeap::new(752);
                heap.enable_gc_stress();
                let mut data = ManagedData::new(752);
                data.configure_values(
                    &heap,
                    Arc::new(
                        ValueSchema::new(vec![
                            ValueType::Scalar(ScalarKind::I64),
                            ValueType::String,
                            ValueType::List(1),
                            ValueType::Map { key: 0, value: 1 },
                            ValueType::Enum {
                                identity: "Option<List<String>>".into(),
                                variants: vec![vec![], vec![2]],
                            },
                            ValueType::List(0),
                            ValueType::Scalar(ScalarKind::U8),
                            ValueType::List(6),
                        ])
                        .unwrap(),
                    ),
                )
                .unwrap();
                let scalar = |n: i64| {
                    ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n as u64).unwrap())
                };
                let map = matches!(op, MapInsert | MapRemove | MapKeys);
                let ty = if map { 3 } else { 2 };
                let mut fields = Vec::new();
                for i in 0..64 {
                    if map {
                        fields.push(scalar(i));
                    }
                    fields.push(ManagedSlot::Reference(
                        data.create_string(&mut heap, &format!("value-{i}🙂"))
                            .unwrap(),
                    ));
                }
                let parent = data.create_composite(&mut heap, ty, None, &fields).unwrap();
                let tail = data.create_string(&mut heap, "tail").unwrap();
                data.retain_roots(&mut heap, [parent, tail]).unwrap();
                let (result, types, slots) = match op {
                    ListAppend => (
                        2,
                        vec![2, 1],
                        vec![ManagedSlot::Reference(parent), ManagedSlot::Reference(tail)],
                    ),
                    ListSet => (
                        4,
                        vec![2, 0, 1],
                        vec![
                            ManagedSlot::Reference(parent),
                            scalar(if missing { -1 } else { 31 }),
                            ManagedSlot::Reference(tail),
                        ],
                    ),
                    ListRemove => (
                        4,
                        vec![2, 0],
                        vec![
                            ManagedSlot::Reference(parent),
                            scalar(if missing { -1 } else { 31 }),
                        ],
                    ),
                    MapInsert => (
                        3,
                        vec![3, 0, 1],
                        vec![
                            ManagedSlot::Reference(parent),
                            scalar(if missing { 99 } else { 31 }),
                            ManagedSlot::Reference(tail),
                        ],
                    ),
                    MapRemove => (
                        3,
                        vec![3, 0],
                        vec![
                            ManagedSlot::Reference(parent),
                            scalar(if missing { 99 } else { 31 }),
                        ],
                    ),
                    MapKeys => (5, vec![3], vec![ManagedSlot::Reference(parent)]),
                    _ => unreachable!(),
                };
                for _ in 0..3 {
                    let ManagedSlot::Reference(output) = data
                        .operation_hashed(&mut heap, op, result, &types, &slots, |_| 0)
                        .unwrap()
                    else {
                        panic!()
                    };
                    assert_eq!(data.root_count(), 3, "{op:?}");
                    heap.collect(crate::heap::CollectionKind::Full).unwrap();
                    data.snapshot(&heap, result, ManagedSlot::Reference(output))
                        .unwrap();
                    data.release(&mut heap, output).unwrap();
                    assert_eq!(data.root_count(), 2);
                }
                assert!(
                    data.operation_hashed(&mut heap, op, 7, &types, &slots, |_| 0)
                        .is_err()
                );
                assert_eq!(data.root_count(), 2, "failed result layout: {op:?}");
                let ManagedSlot::Reference(output) = data
                    .operation(&mut heap, op, result, &types, &slots)
                    .unwrap()
                else {
                    panic!()
                };
                data.release(&mut heap, parent).unwrap();
                data.release(&mut heap, tail).unwrap();
                heap.collect(crate::heap::CollectionKind::Full).unwrap();
                data.snapshot(&heap, result, ManagedSlot::Reference(output))
                    .unwrap();
                data.release(&mut heap, output).unwrap();
                heap.collect(crate::heap::CollectionKind::Full).unwrap();
                assert_eq!((data.root_count(), heap.object_count()), (0, 0));
            }
        }
    }

    #[test]
    fn collection_reads_validate_the_tail_and_only_keep_the_result_root() {
        for op in [DataOperation::ListGet, DataOperation::MapGet] {
            let mut heap = DomainHeap::new(751);
            let mut data = ManagedData::new(751);
            let schema = ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::I64),
                ValueType::String,
                ValueType::List(1),
                ValueType::Map { key: 0, value: 1 },
                ValueType::Enum {
                    identity: "Option<String>".into(),
                    variants: vec![vec![], vec![1]],
                },
            ])
            .unwrap();
            data.configure_values(&heap, Arc::new(schema)).unwrap();
            let integer = |n: i64| {
                ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n as u64).unwrap())
            };
            let mut fields = Vec::new();
            for i in 0..96 {
                if op == DataOperation::MapGet {
                    fields.push(integer(i));
                }
                fields.push(ManagedSlot::Reference(
                    data.create_string(&mut heap, &format!("value-{i}🙂"))
                        .unwrap(),
                ));
            }
            let ty = if op == DataOperation::ListGet { 2 } else { 3 };
            let parent = data.create_composite(&mut heap, ty, None, &fields).unwrap();
            data.retain_roots(&mut heap, [parent]).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            let parent_ref = data
                .reference(&heap, parent, DataKind::Composite(ty))
                .unwrap();
            let tail_index = fields.len() + 1;
            let tail = heap
                .fields_iter(parent_ref)
                .unwrap()
                .nth(tail_index)
                .unwrap();
            heap.write_field(parent_ref, tail_index, HeapValue::Null)
                .unwrap();
            assert_eq!(
                data.operation(
                    &mut heap,
                    op,
                    4,
                    &[ty, 0],
                    &[ManagedSlot::Reference(parent), integer(0)]
                ),
                Err(DataError::InvalidEncoding)
            );
            assert_eq!(data.root_count(), 1);
            heap.write_field(parent_ref, tail_index, tail).unwrap();
            let ManagedSlot::Reference(missing) = data
                .operation(
                    &mut heap,
                    op,
                    4,
                    &[ty, 0],
                    &[ManagedSlot::Reference(parent), integer(-1)],
                )
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(data.enum_variant(&heap, missing, 4).unwrap(), 0);
            assert_eq!(data.root_count(), 2);
            data.release(&mut heap, missing).unwrap();
            let ManagedSlot::Reference(found) = data
                .operation(
                    &mut heap,
                    op,
                    4,
                    &[ty, 0],
                    &[ManagedSlot::Reference(parent), integer(95)],
                )
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(data.root_count(), 2);
            data.release(&mut heap, parent).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(heap.object_count(), 2);
            let Value::Enum {
                variant, fields, ..
            } = data
                .snapshot(&heap, 4, ManagedSlot::Reference(found))
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(variant, 1);
            assert!(matches!(fields.as_slice(), [Value::String(text)] if text == "value-95🙂"));
        }
    }

    #[test]
    fn map_compaction_keeps_first_positions_and_last_pairs_in_reversed_updates() {
        let mut heap = DomainHeap::new(753);
        let mut data = ManagedData::new(753);
        data.configure_values(
            &heap,
            Arc::new(
                ValueSchema::new(vec![
                    ValueType::Scalar(ScalarKind::I64),
                    ValueType::Map { key: 0, value: 0 },
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        let scalar = |n| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n).unwrap());
        let pairs: Vec<_> = (0_u64..32)
            .map(|i| (i, i))
            .chain((0_u64..128).map(|i| (31 - i % 32, i + 1000)))
            .collect();
        let slots: Vec<_> = pairs
            .iter()
            .flat_map(|&(key, value)| [scalar(key), scalar(value)])
            .collect();
        let handle = data
            .create_composite_hashed(&mut heap, 1, None, &slots, |_| 0)
            .unwrap();
        heap.collect(crate::heap::CollectionKind::Full).unwrap();
        let Value::Map(actual) = data
            .snapshot(&heap, 1, ManagedSlot::Reference(handle))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(actual.len(), 32);
        for (index, (key, value)) in actual.into_iter().enumerate() {
            let expected = pairs
                .iter()
                .rev()
                .find(|(key, _)| *key == index as u64)
                .unwrap()
                .1;
            assert!(matches!(key,Value::Scalar(value) if value.bits()==index as u64));
            assert!(matches!(value,Value::Scalar(value) if value.bits()==expected));
        }
    }

    #[test]
    fn forced_hash_collisions_keep_distinct_keys_and_first_positions() {
        let mut heap = DomainHeap::new(9);
        let mut data = ManagedData::new(9);
        let schema = Arc::new(
            ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::I64),
                ValueType::Map { key: 0, value: 0 },
                ValueType::Enum {
                    identity: "Option<Int>".into(),
                    variants: vec![vec![], vec![0]],
                },
            ])
            .unwrap(),
        );
        data.configure_values(&heap, schema).unwrap();
        let scalar = |value: i64| {
            ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value as u64).unwrap())
        };
        let pairs = [
            scalar(1),
            scalar(10),
            scalar(2),
            scalar(20),
            scalar(1),
            scalar(30),
        ];
        let original = data
            .create_composite_hashed(&mut heap, 1, None, &pairs, |_| 0)
            .unwrap();
        let original = ManagedSlot::Reference(original);
        let updated = data
            .operation_hashed(
                &mut heap,
                DataOperation::MapInsert,
                1,
                &[1, 0, 0],
                &[original, scalar(2), scalar(40)],
                |_| 0,
            )
            .unwrap();
        let with_third = data
            .operation_hashed(
                &mut heap,
                DataOperation::MapInsert,
                1,
                &[1, 0, 0],
                &[updated, scalar(3), scalar(50)],
                |_| 0,
            )
            .unwrap();
        let removed = data
            .operation_hashed(
                &mut heap,
                DataOperation::MapRemove,
                1,
                &[1, 0],
                &[with_third, scalar(2)],
                |_| 0,
            )
            .unwrap();
        heap.collect(crate::heap::CollectionKind::Full).unwrap();
        let got = data
            .operation_hashed(
                &mut heap,
                DataOperation::MapGet,
                2,
                &[1, 0],
                &[removed, scalar(3)],
                |_| 0,
            )
            .unwrap();
        let Value::Enum {
            variant, fields, ..
        } = data.snapshot(&heap, 2, got).unwrap()
        else {
            panic!()
        };
        assert_eq!(variant, 1);
        assert!(matches!(&fields[0],Value::Scalar(value) if value.bits()==50));
        let Value::Map(before) = data.snapshot(&heap, 1, original).unwrap() else {
            panic!()
        };
        assert_eq!(before.len(), 2);
        assert!(
            matches!(&before[0],(Value::Scalar(key),Value::Scalar(value)) if key.bits()==1&&value.bits()==30)
        );
        let Value::Map(after) = data.snapshot(&heap, 1, removed).unwrap() else {
            panic!()
        };
        assert_eq!(after.len(), 2);
        assert!(
            matches!(&after[1],(Value::Scalar(key),Value::Scalar(value)) if key.bits()==3&&value.bits()==50)
        );
        data.release_all(&mut heap).unwrap();
        heap.collect(crate::heap::CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 0);
    }

    #[test]
    fn adopted_keys_keep_content_hash_and_equality_under_forced_hash_collisions() {
        let schema = Arc::new(
            ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::I64),
                ValueType::Scalar(ScalarKind::Bool),
                ValueType::Bytes(3),
                ValueType::DynamicBytes,
                ValueType::Record {
                    identity: "ContentKey".into(),
                    fields: vec![1, 2, 3],
                },
                ValueType::Map { key: 1, value: 0 },
                ValueType::Map { key: 2, value: 0 },
                ValueType::Map { key: 3, value: 0 },
                ValueType::Map { key: 4, value: 0 },
                ValueType::Enum {
                    identity: "Option<Int>".into(),
                    variants: vec![vec![], vec![0]],
                },
            ])
            .unwrap(),
        );
        let boolean =
            |value| Value::Scalar(ScalarValue::from_bits(ScalarKind::Bool, value).unwrap());
        let record = |flag| Value::Record {
            identity: "ContentKey".into(),
            fields: vec![
                boolean(flag),
                Value::Bytes(vec![0, 255, 128]),
                Value::Bytes(vec![0, 255]),
            ],
        };
        for (key_type, map_type, keys) in [
            (1, 5, [boolean(0), boolean(1)]),
            (
                2,
                6,
                [
                    Value::Bytes(vec![0, 255, 128]),
                    Value::Bytes(vec![255, 0, 128]),
                ],
            ),
            (
                3,
                7,
                [Value::Bytes(vec![]), Value::Bytes(vec![0, 255, 128])],
            ),
            (4, 8, [record(0), record(1)]),
        ] {
            let mut heap = DomainHeap::new(7);
            let mut data = ManagedData::new(7);
            data.configure_values(&heap, schema.clone()).unwrap();
            let slots = data
                .materialize_values(&mut heap, &[key_type, key_type], &keys)
                .unwrap();
            let scalar = |value| {
                ManagedSlot::Scalar(ScalarValue::from_integer(ScalarKind::I64, value).unwrap())
            };
            let hash = data.value_hash(&heap, key_type, slots[0]).unwrap();
            let original = ManagedSlot::Reference(
                data.create_composite_hashed(
                    &mut heap,
                    map_type,
                    None,
                    &[
                        slots[0],
                        scalar(10),
                        slots[1],
                        scalar(20),
                        slots[0],
                        scalar(30),
                    ],
                    |_| 0,
                )
                .unwrap(),
            );
            let updated = data
                .operation_hashed(
                    &mut heap,
                    DataOperation::MapInsert,
                    map_type,
                    &[map_type, key_type, 0],
                    &[original, slots[1], scalar(40)],
                    |_| 0,
                )
                .unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(hash, data.value_hash(&heap, key_type, slots[0]).unwrap());
            for (index, expected) in [(0, 30), (1, 40)] {
                let got = data
                    .operation_hashed(
                        &mut heap,
                        DataOperation::MapGet,
                        9,
                        &[map_type, key_type],
                        &[updated, slots[index]],
                        |_| 0,
                    )
                    .unwrap();
                let Value::Enum {
                    variant, fields, ..
                } = data.snapshot(&heap, 9, got).unwrap()
                else {
                    panic!()
                };
                assert_eq!(variant, 1);
                assert_eq!(fields.len(), 1);
                assert!(
                    schema
                        .equal(
                            0,
                            &fields[0],
                            &Value::Scalar(
                                ScalarValue::from_integer(ScalarKind::I64, expected).unwrap()
                            )
                        )
                        .unwrap()
                );
            }
            let Value::Map(before) = data.snapshot(&heap, map_type, original).unwrap() else {
                panic!()
            };
            assert_eq!(before.len(), 2);
            assert!(schema.equal(key_type, &before[0].0, &keys[0]).unwrap());
            assert!(schema.equal(key_type, &before[1].0, &keys[1]).unwrap());
            assert!(
                schema
                    .equal(
                        0,
                        &before[1].1,
                        &Value::Scalar(ScalarValue::from_integer(ScalarKind::I64, 20).unwrap())
                    )
                    .unwrap()
            );
            data.release_all(&mut heap).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!((heap.object_count(), data.root_count()), (0, 0));
        }
    }
}
