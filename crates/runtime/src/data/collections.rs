//! Collection construction in private, typed heap objects. Buffers are reachable
//! through ordinary frame roots; unwritten slots are null and index words are
//! integers. No heap address is held across a call or a safepoint.
use super::*;
use crate::scalar::{ScalarKind, ScalarValue};
use crate::value::ValueType;
use rinzu_abi::data_operation::DataOperation;

const HEADER: usize = 2; // schema type, used slot count
const MAP_WORDS: usize = 6; // two slots and two (position, hash) buckets per pair

pub(super) fn added_length(
    operation: DataOperation,
    current: usize,
    added: usize,
) -> Result<usize, DataError> {
    let current = i64::try_from(current).map_err(|_| DataError::InvalidEncoding)?;
    let added = i64::try_from(added).map_err(|_| DataError::InvalidEncoding)?;
    let total = current
        .checked_add(added)
        .ok_or(DataError::CollectionOverflow {
            operation,
            current,
            added,
        })?;
    usize::try_from(total).map_err(|_| DataError::AddressSpaceExhausted)
}

#[derive(Clone, Copy)]
struct Buffer {
    reference: HeapRef,
    used: usize,
    capacity: usize,
    map: bool,
}

impl Buffer {
    fn get(heap: &DomainHeap, reference: HeapRef, ty: usize, map: bool) -> Result<Self, DataError> {
        let header = heap.header(reference)?;
        let available = (header.field_count as usize)
            .checked_sub(HEADER)
            .ok_or(DataError::InvalidEncoding)?;
        if usize::try_from(heap.integer_field(reference, 0)?).ok() != Some(ty) {
            return Err(DataError::WrongKind);
        }
        let metadata = heap.integer_field(reference, 1)?;
        let (used, capacity) = match header.type_id {
            COMPOSITE_TYPE if metadata == -1 && available == 0 => (0, 0),
            COLLECTION_BUFFER_TYPE => {
                let used = usize::try_from(metadata).map_err(|_| DataError::InvalidEncoding)?;
                if map {
                    let capacity = available / MAP_WORDS;
                    if !available.is_multiple_of(MAP_WORDS)
                        || !capacity.is_power_of_two()
                        || !used.is_multiple_of(2)
                        || used > 2 * capacity
                    {
                        return Err(DataError::InvalidEncoding);
                    }
                    (used, capacity)
                } else {
                    if used > available {
                        return Err(DataError::InvalidEncoding);
                    }
                    (used, available)
                }
            }
            _ => return Err(DataError::WrongKind),
        };
        Ok(Self {
            reference,
            used,
            capacity,
            map,
        })
    }

    fn grow(self, heap: &mut DomainHeap, ty: usize, capacity: usize) -> Result<Self, DataError> {
        let count = capacity
            .checked_mul(if self.map { MAP_WORDS } else { 1 })
            .and_then(|count| count.checked_add(HEADER))
            .ok_or(DataError::AddressSpaceExhausted)?;
        let reference = heap.allocate_null_fields(COLLECTION_BUFFER_TYPE, count)?;
        let grown = Self {
            reference,
            capacity,
            ..self
        };
        heap.write_field(reference, 0, HeapValue::Integer(ty as i64))?;
        heap.write_field(reference, 1, HeapValue::Integer(self.used as i64))?;
        heap.copy_fields(self.reference, HEADER, reference, HEADER, self.used)?;
        if self.map {
            for bucket in 0..2 * self.capacity {
                if let Some((position, hash)) = self.bucket(heap, bucket)? {
                    grown.index(heap, position, hash)?;
                }
            }
        }
        Ok(grown)
    }

    fn bucket(self, heap: &DomainHeap, bucket: usize) -> Result<Option<(usize, u64)>, DataError> {
        let offset = HEADER + 2 * (self.capacity + bucket);
        match heap.field_at(self.reference, offset)? {
            HeapValue::Null => Ok(None),
            HeapValue::Integer(position) => Ok(Some((
                usize::try_from(position).map_err(|_| DataError::InvalidEncoding)?,
                heap.integer_field(self.reference, offset + 1)? as u64,
            ))),
            _ => Err(DataError::InvalidEncoding),
        }
    }

    fn index(self, heap: &mut DomainHeap, position: usize, hash: u64) -> Result<(), DataError> {
        let mask = 2 * self.capacity - 1;
        let mut bucket = hash as usize & mask;
        while self.bucket(heap, bucket)?.is_some() {
            bucket = (bucket + 1) & mask;
        }
        let offset = HEADER + 2 * (self.capacity + bucket);
        heap.write_field(self.reference, offset, HeapValue::Integer(position as i64))?;
        heap.write_field(self.reference, offset + 1, HeapValue::Integer(hash as i64))?;
        Ok(())
    }

    fn finish(self, heap: &mut DomainHeap, ty: usize) -> Result<HeapRef, DataError> {
        let reference = allocate_collection(heap, ty, self.used)?;
        heap.copy_fields(self.reference, HEADER, reference, HEADER, self.used)?;
        Ok(reference)
    }
}

fn allocate_collection(
    heap: &mut DomainHeap,
    ty: usize,
    slots: usize,
) -> Result<HeapRef, DataError> {
    let count = slots
        .checked_add(HEADER)
        .ok_or(DataError::AddressSpaceExhausted)?;
    let reference = heap.allocate_null_fields(COMPOSITE_TYPE, count)?;
    heap.write_field(reference, 0, HeapValue::Integer(ty as i64))?;
    heap.write_field(reference, 1, HeapValue::Integer(-1))?;
    Ok(reference)
}

impl ManagedData {
    pub(super) fn buffer_fields<'a>(
        &self,
        heap: &'a DomainHeap,
        reference: HeapRef,
        ty: usize,
    ) -> Result<std::iter::Take<crate::heap::HeapFields<'a>>, DataError> {
        let map = matches!(self.value_schema()?.ty(ty)?, ValueType::Map { .. });
        let buffer = Buffer::get(heap, reference, ty, map)?;
        let mut fields = heap.fields_iter(reference)?;
        fields.nth(HEADER - 1).ok_or(DataError::InvalidEncoding)?;
        Ok(fields.take(buffer.used))
    }

    pub(super) fn collection_construction(
        &mut self,
        heap: &mut DomainHeap,
        operation: DataOperation,
        result: usize,
        input: usize,
        slots: &[ManagedSlot],
        remap_hash: fn(u64) -> u64,
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        let ManagedSlot::Reference(handle) = slots[0] else {
            return Err(DataError::WrongKind);
        };
        let source = self.reference(heap, handle, DataKind::Composite(input))?;
        match operation {
            ListConcatAll => return self.concat_lists(heap, result, input, source),
            MapEntries => return self.map_entries(heap, result, input, source),
            _ => {}
        }
        let layout = self.value_schema()?.ty(input)?.clone();
        let mut buffer = Buffer::get(heap, source, input, matches!(layout, ValueType::Map { .. }))?;
        match (operation, layout) {
            (CollectionFinish, ValueType::List(_) | ValueType::Map { .. }) if result == input => {
                if buffer.capacity == 0 {
                    return Ok(slots[0]);
                }
                let reference = buffer.finish(heap, result)?;
                return self
                    .root(heap, reference, DataKind::Composite(result))
                    .map(ManagedSlot::Reference);
            }
            (ListBufferReserve, ValueType::List(_)) if input == result && buffer.capacity == 0 => {
                let ManagedSlot::Scalar(length) = slots[1] else {
                    return Err(DataError::WrongKind);
                };
                let length = usize::try_from(length.bits() as i64)
                    .map_err(|_| DataError::AddressSpaceExhausted)?;
                if length != 0 {
                    buffer = buffer.grow(heap, input, length)?;
                }
            }
            (ListBufferAppend, ValueType::List(element)) if input == result => {
                let used = added_length(operation, buffer.used, 1)?;
                let field = self.slot_field(heap, element, slots[1])?;
                if used > buffer.capacity {
                    buffer = buffer.grow(heap, input, doubled(buffer.capacity)?)?;
                }
                heap.write_field(buffer.reference, HEADER + buffer.used, field)?;
                heap.write_field(buffer.reference, 1, HeapValue::Integer(used as i64))?;
            }
            (MapBufferIndex | MapBufferInsert, ValueType::Map { key, value }) => {
                let schema = self.value_schema()?;
                let wanted = self.snapshot(heap, key, slots[1])?;
                let hash = remap_hash(schema.key_hash(key, &wanted)?);
                let mut selected = None;
                if buffer.capacity != 0 {
                    let mask = 2 * buffer.capacity - 1;
                    let mut cursor = hash as usize & mask;
                    while let Some((position, previous_hash)) = buffer.bucket(heap, cursor)? {
                        if previous_hash == hash {
                            let field = heap.field_at(buffer.reference, HEADER + position)?;
                            let previous = self.snapshot_field(heap, key, field)?;
                            if schema.equal(key, &previous, &wanted)? {
                                selected = Some(position);
                                break;
                            }
                        }
                        cursor = (cursor + 1) & mask;
                    }
                }
                if operation == MapBufferIndex {
                    let index = selected.map_or(-1, |position| (position / 2) as i64);
                    return Ok(ManagedSlot::Scalar(
                        ScalarValue::from_bits(ScalarKind::I64, index as u64).expect("Int index"),
                    ));
                }
                if input != result {
                    return Err(DataError::WrongKind);
                }
                let replacement = self.slot_field(heap, value, slots[2])?;
                if let Some(position) = selected {
                    heap.write_field(buffer.reference, HEADER + position + 1, replacement)?;
                } else {
                    let pairs = added_length(operation, buffer.used / 2, 1)?;
                    if pairs > buffer.capacity {
                        buffer = buffer.grow(heap, input, doubled(buffer.capacity)?)?;
                    }
                    let field = self.slot_field(heap, key, slots[1])?;
                    heap.write_field(buffer.reference, HEADER + buffer.used, field)?;
                    heap.write_field(buffer.reference, HEADER + buffer.used + 1, replacement)?;
                    buffer.index(heap, buffer.used, hash)?;
                    heap.write_field(buffer.reference, 1, HeapValue::Integer((pairs * 2) as i64))?;
                }
            }
            _ => return Err(DataError::WrongKind),
        }
        if buffer.reference == source {
            Ok(slots[0])
        } else {
            self.root(heap, buffer.reference, DataKind::Composite(input))
                .map(ManagedSlot::Reference)
        }
    }

    fn concat_lists(
        &mut self,
        heap: &mut DomainHeap,
        result: usize,
        input: usize,
        source: HeapRef,
    ) -> Result<ManagedSlot, DataError> {
        let schema = self.value_schema()?;
        if !matches!(schema.ty(input)?, ValueType::List(part) if *part == result)
            || !matches!(schema.ty(result)?, ValueType::List(_))
        {
            return Err(DataError::WrongKind);
        }
        let parts = list_slot_count(heap, source, input)?;
        let part = |heap: &DomainHeap, index| {
            let HeapValue::Reference(reference) = heap.field_at(source, HEADER + index)? else {
                return Err(DataError::WrongKind);
            };
            Ok((reference, list_slot_count(heap, reference, result)?))
        };
        let mut length = 0;
        for index in 0..parts {
            let (_, count) = part(heap, index)?;
            length = added_length(DataOperation::ListConcatAll, length, count)?;
        }
        let output = allocate_collection(heap, result, length)?;
        let mut offset = HEADER;
        for index in 0..parts {
            let (reference, count) = part(heap, index)?;
            heap.copy_fields(reference, HEADER, output, offset, count)?;
            offset += count;
        }
        self.root(heap, output, DataKind::Composite(result))
            .map(ManagedSlot::Reference)
    }

    fn map_entries(
        &mut self,
        heap: &mut DomainHeap,
        result: usize,
        input: usize,
        source: HeapRef,
    ) -> Result<ManagedSlot, DataError> {
        let schema = self.value_schema()?;
        let ValueType::Map { key, value } = *schema.ty(input)? else {
            return Err(DataError::WrongKind);
        };
        let ValueType::List(entry) = *schema.ty(result)? else {
            return Err(DataError::WrongKind);
        };
        if !matches!(schema.ty(entry)?, ValueType::Record { fields, .. } if fields.as_slice() == [key, value])
        {
            return Err(DataError::WrongKind);
        }
        let slots = list_slot_count(heap, source, input)?;
        if !slots.is_multiple_of(2) {
            return Err(DataError::InvalidEncoding);
        }
        let output = allocate_collection(heap, result, slots / 2)?;
        for index in 0..slots / 2 {
            let record = allocate_collection(heap, entry, 2)?;
            heap.copy_fields(source, HEADER + index * 2, record, HEADER, 2)?;
            heap.write_field(output, HEADER + index, HeapValue::Reference(record))?;
        }
        self.root(heap, output, DataKind::Composite(result))
            .map(ManagedSlot::Reference)
    }
}

// The schema and constructor have checked every child of an immutable collection.
fn list_slot_count(heap: &DomainHeap, source: HeapRef, ty: usize) -> Result<usize, DataError> {
    let header = heap.header(source)?;
    if header.type_id != COMPOSITE_TYPE
        || usize::try_from(heap.integer_field(source, 0)?).ok() != Some(ty)
        || heap.integer_field(source, 1)? != -1
    {
        return Err(DataError::WrongKind);
    }
    (header.field_count as usize)
        .checked_sub(HEADER)
        .ok_or(DataError::InvalidEncoding)
}

fn doubled(capacity: usize) -> Result<usize, DataError> {
    if capacity == 0 {
        return Ok(4);
    }
    capacity
        .checked_mul(2)
        .ok_or(DataError::AddressSpaceExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::ValueSchema;
    use std::{cell::Cell, collections::BTreeSet, sync::Arc};

    thread_local! {
        static HASHES: Cell<usize> = const { Cell::new(0) };
    }

    fn count_hash(hash: u64) -> u64 {
        HASHES.set(HASHES.get() + 1);
        hash
    }

    fn int(value: i64) -> ManagedSlot {
        ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value as u64).unwrap())
    }

    #[test]
    fn checked_lengths_use_the_public_signed_limit() {
        use DataOperation::*;
        let limit = i64::MAX as usize;
        for operation in [
            ListAppend,
            MapInsert,
            ListConcatAll,
            ListBufferAppend,
            MapBufferInsert,
        ] {
            assert_eq!(added_length(operation, limit - 1, 1), Ok(limit));
            assert_eq!(added_length(operation, limit, 0), Ok(limit));
            assert_eq!(
                added_length(operation, limit, 1),
                Err(DataError::CollectionOverflow {
                    operation,
                    current: i64::MAX,
                    added: 1,
                })
            );
        }
        assert_eq!(
            added_length(ListConcatAll, limit - 1, 2),
            Err(DataError::CollectionOverflow {
                operation: ListConcatAll,
                current: i64::MAX - 1,
                added: 2,
            })
        );
    }

    #[test]
    fn buffer_growth_moves_fewer_than_two_prefixes_and_never_rehashes_old_keys() {
        use DataOperation::*;
        for count in [1024, 2048] {
            for (ty, op, stride) in [(1, ListBufferAppend, 1), (2, MapBufferInsert, 2)] {
                let mut heap = DomainHeap::new(101);
                let mut data = ManagedData::new(101);
                data.configure_values(
                    &heap,
                    Arc::new(
                        ValueSchema::new(vec![
                            ValueType::Scalar(ScalarKind::I64),
                            ValueType::List(0),
                            ValueType::Map { key: 0, value: 0 },
                        ])
                        .unwrap(),
                    ),
                )
                .unwrap();
                let empty = data.create_composite(&mut heap, ty, None, &[]).unwrap();
                let mut buffer = ManagedSlot::Reference(empty);
                HASHES.set(0);
                for index in 0..count {
                    let types = [ty, 0, 0];
                    let slots = [buffer, int(index as i64), int(index as i64)];
                    buffer = data
                        .operation_hashed(
                            &mut heap,
                            op,
                            ty,
                            &types[..op.arity()],
                            &slots[..op.arity()],
                            count_hash,
                        )
                        .unwrap();
                }
                assert_eq!(heap.copied_fields, (count - 4) * stride);
                assert_eq!(heap.object_count(), 1 + count.ilog2() as usize - 1);
                if op == MapBufferInsert {
                    let before = heap.object_count();
                    for value in 0..count {
                        let unchanged = data
                            .operation_hashed(
                                &mut heap,
                                op,
                                ty,
                                &[ty, 0, 0],
                                &[buffer, int(3), int(value as i64)],
                                count_hash,
                            )
                            .unwrap();
                        assert_eq!(unchanged, buffer);
                    }
                    assert_eq!(heap.object_count(), before);
                    assert_eq!(heap.copied_fields, (count - 4) * stride);
                    assert_eq!(HASHES.get(), count * 2);
                }
                let ManagedSlot::Reference(result) = data
                    .operation(&mut heap, CollectionFinish, ty, &[ty], &[buffer])
                    .unwrap()
                else {
                    panic!("collection result");
                };
                assert_eq!(heap.copied_fields, (2 * count - 4) * stride);
                assert_eq!(data.collection_length(&heap, result, ty), Ok(count));
                assert_eq!(data.collection_length(&heap, empty, ty), Ok(0));
                let result = data
                    .reference(&heap, result, DataKind::Composite(ty))
                    .unwrap();
                for index in 0..count {
                    assert_eq!(
                        heap.integer_field(result, HEADER + index * stride),
                        Ok(index as i64)
                    );
                }
                if op == MapBufferInsert {
                    assert_eq!(
                        heap.integer_field(result, HEADER + 3 * 2 + 1),
                        Ok(count as i64 - 1)
                    );
                }
            }
        }
    }

    #[test]
    fn collisions_and_moving_gc_keep_only_the_typed_initialized_slots() {
        use DataOperation::*;
        let mut heap = DomainHeap::new(102);
        let mut data = ManagedData::new(102);
        data.configure_values(
            &heap,
            Arc::new(
                ValueSchema::new(vec![
                    ValueType::Scalar(ScalarKind::I64),
                    ValueType::Other,
                    ValueType::Map { key: 0, value: 1 },
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        let empty = data.create_composite(&mut heap, 2, None, &[]).unwrap();
        let mut buffer = ManagedSlot::Reference(empty);
        for index in 0..17 {
            buffer = data
                .operation_hashed(
                    &mut heap,
                    MapBufferInsert,
                    2,
                    &[2, 0, 1],
                    &[buffer, int(index), ManagedSlot::Opaque(100 + index as u64)],
                    |_| 1,
                )
                .unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
        }
        let ManagedSlot::Reference(root) = buffer else {
            panic!("map buffer");
        };
        let futures = data
            .reachable_futures(&heap, [root], [], &BTreeSet::from([1]), &BTreeMap::new())
            .unwrap();
        assert_eq!(futures, (100..117).collect());
        let selected = data
            .operation_hashed(
                &mut heap,
                MapBufferIndex,
                0,
                &[2, 0],
                &[buffer, int(12)],
                |_| 1,
            )
            .unwrap();
        assert_eq!(selected, int(12));
        let selected = data
            .operation_hashed(
                &mut heap,
                MapBufferIndex,
                0,
                &[2, 0],
                &[buffer, int(91)],
                |_| 1,
            )
            .unwrap();
        assert_eq!(selected, int(-1));
    }
}
