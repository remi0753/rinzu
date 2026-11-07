use super::composite::DecodedSlot;
use super::*;
use crate::scalar::{ScalarKind, ScalarValue};
use crate::value::{Value, ValueType};
use rinzu_abi::data_operation::DataOperation;

fn packed_bytes_from(
    fields: crate::heap::HeapFields<'_>,
    length: usize,
    start: usize,
) -> impl Iterator<Item = u8> + '_ {
    fields
        .skip(start / 8)
        .flat_map(|field| {
            (integer(Some(&field)).expect("checked packed integer") as u64).to_le_bytes()
        })
        .skip(start % 8)
        .take(length.saturating_sub(start))
}

fn find_packed_byte(
    fields: crate::heap::HeapFields<'_>,
    length: usize,
    start: usize,
    members: &[bool; 256],
    in_set: bool,
) -> Option<usize> {
    if start >= length {
        return None;
    }
    let mut fields = fields.skip(start / 8);
    let mut base = start / 8 * 8;
    if !start.is_multiple_of(8) {
        let word =
            (integer(fields.next().as_ref()).expect("checked packed integer") as u64).to_le_bytes();
        for offset in start % 8..(length - base).min(8) {
            if members[word[offset] as usize] == in_set {
                return Some(base + offset);
            }
        }
        base += 8;
    }
    while length.saturating_sub(base) >= 8 {
        let word =
            (integer(fields.next().as_ref()).expect("checked packed integer") as u64).to_le_bytes();
        // The common loop has a fixed eight bytes, without a flat iterator's
        // per-byte state machine or per-byte end-of-payload checks.
        for (offset, byte) in word.into_iter().enumerate() {
            if members[byte as usize] == in_set {
                return Some(base + offset);
            }
        }
        base += 8;
    }
    if base < length {
        let word =
            (integer(fields.next().as_ref()).expect("checked packed integer") as u64).to_le_bytes();
        for (offset, byte) in word.into_iter().take(length - base).enumerate() {
            if members[byte as usize] == in_set {
                return Some(base + offset);
            }
        }
    }
    None
}

impl ManagedData {
    pub(super) fn bytes_operation(
        &mut self,
        heap: &mut DomainHeap,
        op: DataOperation,
        result: usize,
        types: &[usize],
        slots: &[ManagedSlot],
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        if op == BytesConcatAll {
            let ValueType::List(child) = self.value_schema()?.ty(types[0])? else {
                return Err(DataError::WrongKind);
            };
            let child = *child;
            if !matches!(self.value_schema()?.ty(child)?, ValueType::DynamicBytes)
                || !matches!(self.value_schema()?.ty(result)?, ValueType::DynamicBytes)
            {
                return Err(DataError::WrongKind);
            }
            let ManagedSlot::Reference(handle) = slots[0] else {
                return Err(DataError::WrongKind);
            };
            let reference = self.reference(heap, handle, DataKind::Composite(types[0]))?;
            let (variant, fields) = self.composite_fields_iter(heap, reference, types[0])?;
            if variant.is_some() {
                return Err(DataError::WrongKind);
            }
            let mut parts = Vec::new();
            parts
                .try_reserve_exact(fields.len())
                .unwrap_or_else(|_| std::process::abort());
            let mut total = 0usize;
            let mut nonempty = 0usize;
            let mut shared = None;
            for field in fields {
                let DecodedSlot::Reference(reference, kind) =
                    self.decode_slot_field(heap, child, field)?
                else {
                    return Err(DataError::WrongKind);
                };
                let (length, _) = self.checked_byte_reference_fields(heap, reference, kind)?;
                total = total
                    .checked_add(length)
                    .filter(|n| *n <= i64::MAX as usize)
                    .ok_or(DataError::AddressSpaceExhausted)?;
                if length != 0 {
                    nonempty += 1;
                    shared = Some((reference, kind));
                }
                parts.push((reference, length));
            }
            if total == 0 {
                shared = parts
                    .first()
                    .map(|(reference, _)| (*reference, DataKind::Bytes(0)));
            }
            if nonempty <= 1
                && let Some((reference, kind)) = shared
            {
                return self.root(heap, reference, kind).map(ManagedSlot::Reference);
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(total)
                .unwrap_or_else(|_| std::process::abort());
            // No allocation/GC can move these borrowed inputs during copying.
            for (reference, length) in parts {
                let mut fields = heap.fields_iter(reference)?;
                fields.next();
                bytes.extend(packed_bytes_from(fields, length, 0));
            }
            return self
                .create_bytes(heap, total as u64, &bytes)
                .map(ManagedSlot::Reference);
        }
        if op == BytesLength {
            let ManagedSlot::Reference(handle) = slots[0] else {
                return Err(DataError::WrongKind);
            };
            let n = self.byte_length(heap, handle, u64::MAX)?;
            return Ok(ManagedSlot::Scalar(
                ScalarValue::from_bits(ScalarKind::I64, n)
                    .map_err(|_| DataError::InvalidEncoding)?,
            ));
        }
        if op == BytesTrim {
            if !matches!(self.value_schema()?.ty(result)?, ValueType::DynamicBytes) {
                return Err(DataError::WrongKind);
            }
            let (ManagedSlot::Reference(input), ManagedSlot::Reference(set)) = (slots[0], slots[1])
            else {
                return Err(DataError::WrongKind);
            };
            let (length, fields) = self.checked_byte_fields(heap, input)?;
            let (set_length, set_fields) = self.checked_byte_fields(heap, set)?;
            let mut members = [false; 256];
            for byte in packed_bytes_from(set_fields, set_length, 0) {
                members[byte as usize] = true;
            }
            let mut start = 0;
            'leading: for field in fields.clone() {
                let word =
                    (integer(Some(&field)).expect("checked packed integer") as u64).to_le_bytes();
                for byte in word.into_iter().take(length - start) {
                    if !members[byte as usize] {
                        break 'leading;
                    }
                    start += 1;
                }
            }
            let mut end = length;
            let mut tail = fields.clone();
            'trailing: while end > start {
                let word = (integer(tail.next_back().as_ref()).expect("checked packed integer")
                    as u64)
                    .to_le_bytes();
                for offset in (0..=(end - 1) % 8).rev() {
                    if end == start || !members[word[offset] as usize] {
                        break 'trailing;
                    }
                    end -= 1;
                }
            }
            if start == 0 && end == length {
                let (reference, kind) = self.any_reference(heap, input)?;
                return self.root(heap, reference, kind).map(ManagedSlot::Reference);
            }
            let bytes: Vec<u8> = packed_bytes_from(fields, length, start)
                .take(end - start)
                .collect();
            return self
                .create_bytes(heap, bytes.len() as u64, &bytes)
                .map(ManagedSlot::Reference);
        }
        if matches!(op, BytesContainsAny | BytesAllIn) {
            if !matches!(
                self.value_schema()?.ty(result)?,
                ValueType::Scalar(ScalarKind::Bool)
            ) {
                return Err(DataError::WrongKind);
            }
            let (ManagedSlot::Reference(input), ManagedSlot::Reference(set)) = (slots[0], slots[1])
            else {
                return Err(DataError::WrongKind);
            };
            // Validate both entire payloads before an early hit, miss or empty result.
            let (length, fields) = self.checked_byte_fields(heap, input)?;
            let (set_length, set_fields) = self.checked_byte_fields(heap, set)?;
            let mut members = [false; 256];
            for byte in packed_bytes_from(set_fields, set_length, 0) {
                members[byte as usize] = true;
            }
            let found =
                find_packed_byte(fields, length, 0, &members, op == BytesContainsAny).is_some();
            let value = if op == BytesContainsAny {
                found
            } else {
                !found
            };
            return Ok(ManagedSlot::Scalar(
                ScalarValue::from_bits(ScalarKind::Bool, u64::from(value)).unwrap(),
            ));
        }
        if matches!(op, BytesGet | BytesFindAny | BytesFindNotAny | BytesSlice) {
            return self.bytes_query(heap, op, result, slots);
        }
        let list_u8 = if op == ByteArray {
            if !matches!(self.value_schema()?.ty(result)?, ValueType::DynamicBytes) {
                return Err(DataError::WrongKind);
            }
            match self.value_schema()?.ty(types[0])? {
                ValueType::Bytes(_) | ValueType::DynamicBytes => {
                    let ManagedSlot::Reference(handle) = slots[0] else {
                        return Err(DataError::WrongKind);
                    };
                    let (reference, kind) = self.any_reference(heap, handle)?;
                    self.checked_byte_reference_fields(heap, reference, kind)?;
                    // A distinct root lets the caller release the source handle;
                    // the immutable packed payload needs no second allocation.
                    return self.root(heap, reference, kind).map(ManagedSlot::Reference);
                }
                ValueType::List(child)
                    if matches!(
                        self.value_schema()?.ty(*child)?,
                        ValueType::Scalar(ScalarKind::U8)
                    ) =>
                {
                    Some(*child)
                }
                _ => None,
            }
        } else {
            None
        };
        let bytes = if let Some(child) = list_u8 {
            let ManagedSlot::Reference(handle) = slots[0] else {
                return Err(DataError::WrongKind);
            };
            let reference = self.reference(heap, handle, DataKind::Composite(types[0]))?;
            let (_, fields) = self.composite_fields_iter(heap, reference, types[0])?;
            let mut bytes = Vec::with_capacity(fields.len());
            for field in fields {
                let DecodedSlot::Value(ManagedSlot::Scalar(value)) =
                    self.decode_slot_field(heap, child, field)?
                else {
                    return Err(DataError::WrongKind);
                };
                bytes.push(value.bits() as u8);
            }
            bytes
        } else {
            match self.snapshot(heap, types[0], slots[0])? {
                Value::Bytes(bytes) => bytes,
                Value::List(values) if op == ByteArray => values
                    .into_iter()
                    .map(|v| match v {
                        Value::Scalar(v) if v.kind() == ScalarKind::U8 => Ok(v.bits() as u8),
                        _ => Err(DataError::WrongKind),
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(DataError::WrongKind),
            }
        };
        let mut bytes = bytes;
        if op == BytesConcat {
            let Value::Bytes(other) = self.snapshot(heap, types[1], slots[1])? else {
                return Err(DataError::WrongKind);
            };
            let length = bytes
                .len()
                .checked_add(other.len())
                .filter(|n| *n <= i64::MAX as usize)
                .ok_or(DataError::AddressSpaceExhausted)?;
            bytes
                .try_reserve_exact(length - bytes.len())
                .unwrap_or_else(|_| std::process::abort());
            bytes.extend_from_slice(&other);
        } else if op == BytesXor {
            let ManagedSlot::Scalar(mask) = slots[1] else {
                return Err(DataError::WrongKind);
            };
            if mask.kind() != ScalarKind::U8 {
                return Err(DataError::WrongKind);
            }
            for byte in &mut bytes {
                *byte ^= mask.bits() as u8;
            }
        } else if op == BytesLowerAscii {
            bytes.make_ascii_lowercase();
        } else if !matches!(op, ByteArray | BytesCopy) {
            return Err(DataError::WrongKind);
        }
        if !matches!(self.value_schema()?.ty(result)?, ValueType::DynamicBytes) {
            return Err(DataError::WrongKind);
        }
        Ok(ManagedSlot::Reference(self.create_bytes(
            heap,
            bytes.len() as u64,
            &bytes,
        )?))
    }

    fn bytes_query(
        &mut self,
        heap: &mut DomainHeap,
        op: DataOperation,
        result: usize,
        slots: &[ManagedSlot],
    ) -> Result<ManagedSlot, DataError> {
        use DataOperation::*;
        let ManagedSlot::Reference(handle) = slots[0] else {
            return Err(DataError::WrongKind);
        };
        let (length, fields) = self.checked_byte_fields(heap, handle)?;
        let index = |i: usize| -> Result<Option<usize>, DataError> {
            match slots[i] {
                ManagedSlot::Scalar(v) if v.kind() == ScalarKind::I64 => {
                    Ok(usize::try_from(v.bits() as i64).ok())
                }
                _ => Err(DataError::WrongKind),
            }
        };
        let value = match op {
            BytesFindAny | BytesFindNotAny => {
                let ManagedSlot::Reference(set) = slots[1] else {
                    return Err(DataError::WrongKind);
                };
                let (set_length, set) = self.checked_byte_fields(heap, set)?;
                let mut members = [false; 256];
                let mut remaining = set_length;
                for field in set {
                    let word = (integer(Some(&field)).expect("checked packed integer") as u64)
                        .to_le_bytes();
                    let count = remaining.min(8);
                    for byte in &word[..count] {
                        members[*byte as usize] = true;
                    }
                    remaining -= count;
                }
                index(2)?
                    .filter(|start| *start <= length)
                    .and_then(|start| {
                        find_packed_byte(fields, length, start, &members, op == BytesFindAny).map(
                            |offset| {
                                ManagedSlot::Scalar(
                                    ScalarValue::from_bits(ScalarKind::I64, offset as u64).unwrap(),
                                )
                            },
                        )
                    })
            }
            BytesGet => index(1)?.filter(|i| *i < length).map(|i| {
                let field = fields.clone().nth(i / 8).expect("checked byte offset");
                let word = integer(Some(&field)).expect("checked packed integer") as u64;
                ManagedSlot::Scalar(
                    ScalarValue::from_bits(ScalarKind::U8, (word >> (i % 8 * 8)) & 0xff).unwrap(),
                )
            }),
            BytesSlice => {
                let interval = index(1)?
                    .zip(index(2)?)
                    .filter(|(a, b)| a <= b && *b <= length);
                if let Some((a, b)) = interval {
                    let mut bytes = Vec::with_capacity(b - a);
                    bytes.extend(packed_bytes_from(fields, length, a).take(b - a));
                    Some(ManagedSlot::Reference(self.create_bytes(
                        heap,
                        (b - a) as u64,
                        &bytes,
                    )?))
                } else {
                    None
                }
            }
            _ => unreachable!("byte query dispatcher"),
        };
        let output = self.create_composite(
            heap,
            result,
            Some(u32::from(value.is_some())),
            value.as_slice(),
        );
        // The returned Option keeps a slice alive through its transitive edge.
        // Release the internal child alias on both success and layout failure.
        if let Some(ManagedSlot::Reference(handle)) = value {
            self.release(heap, handle)?;
        }
        output.map(ManagedSlot::Reference)
    }
}

#[cfg(test)]
mod scan_tests {
    use super::*;
    use crate::value::ValueSchema;
    use std::sync::Arc;

    fn setup() -> (DomainHeap, ManagedData) {
        let heap = DomainHeap::new(312);
        let mut data = ManagedData::new(312);
        data.configure_values(
            &heap,
            Arc::new(
                ValueSchema::new(vec![
                    ValueType::Scalar(ScalarKind::I64),
                    ValueType::DynamicBytes,
                    ValueType::Enum {
                        identity: "Option<Int>".into(),
                        variants: vec![vec![], vec![0]],
                    },
                    ValueType::Scalar(ScalarKind::U8),
                    ValueType::Enum {
                        identity: "Option<U8>".into(),
                        variants: vec![vec![], vec![3]],
                    },
                    ValueType::Enum {
                        identity: "Option<Bytes>".into(),
                        variants: vec![vec![], vec![1]],
                    },
                    ValueType::List(3),
                    ValueType::Scalar(ScalarKind::Bool),
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        (heap, data)
    }
    fn index(value: i64) -> ManagedSlot {
        ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value as u64).unwrap())
    }

    // Tagged payloads allow testing invalid field tags separately from the
    // packed integer representation used by ordinary byte construction.
    fn tagged_bytes(
        heap: &mut DomainHeap,
        data: &mut ManagedData,
        bytes: &[u8],
    ) -> (ManagedHandle, HeapRef) {
        let fields = PackedWords::new(bytes)
            .unwrap()
            .map(HeapValue::Integer)
            .collect::<Vec<_>>();
        let reference = heap
            .allocate(BYTES_TYPE, &fields, FinalizerPolicy::None)
            .unwrap();
        let handle = data
            .root(heap, reference, DataKind::Bytes(bytes.len() as u64))
            .unwrap();
        (handle, reference)
    }

    #[test]
    fn concat_all_keeps_binary_order_aliases_and_children_after_parent_gc() {
        for count in [0, 1, 2, 16, 65] {
            let mut heap = DomainHeap::new(314);
            let mut data = ManagedData::new(314);
            data.configure_values(
                &heap,
                Arc::new(
                    ValueSchema::new(vec![ValueType::DynamicBytes, ValueType::List(0)]).unwrap(),
                ),
            )
            .unwrap();
            let mut expected = Vec::new();
            let mut handles = Vec::new();
            for index in 0..count {
                let bytes = match index % 4 {
                    0 => vec![],
                    1 => vec![0, 255, 128, 0],
                    2 => (0..=255).collect(),
                    _ => vec![b'x'; 8193],
                };
                expected.extend_from_slice(&bytes);
                handles.push(
                    data.create_bytes(&mut heap, bytes.len() as u64, &bytes)
                        .unwrap(),
                );
            }
            let slots: Vec<_> = handles
                .iter()
                .copied()
                .map(ManagedSlot::Reference)
                .collect();
            let list = data.create_composite(&mut heap, 1, None, &slots).unwrap();
            for handle in handles {
                data.release(&mut heap, handle).unwrap();
            }
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            let result = data
                .operation(
                    &mut heap,
                    DataOperation::BytesConcatAll,
                    0,
                    &[1],
                    &[ManagedSlot::Reference(list)],
                )
                .unwrap();
            data.release(&mut heap, list).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(data.root_count(), 1);
            let Value::Bytes(actual) = data.snapshot(&heap, 0, result).unwrap() else {
                panic!("bytes value");
            };
            assert_eq!(actual, expected);
            let ManagedSlot::Reference(handle) = result else {
                panic!("bytes reference");
            };
            data.release(&mut heap, handle).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(data.root_count(), 0);
            assert_eq!(heap.object_count(), 0);
        }
    }

    #[test]
    fn concat_all_rejects_wrong_list_result_and_corrupt_packed_child() {
        let mut heap = DomainHeap::new(315);
        let mut data = ManagedData::new(315);
        data.configure_values(
            &heap,
            Arc::new(
                ValueSchema::new(vec![
                    ValueType::DynamicBytes,
                    ValueType::List(0),
                    ValueType::Scalar(ScalarKind::I64),
                    ValueType::List(2),
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        let wrong = data
            .create_composite(&mut heap, 3, None, &[index(1)])
            .unwrap();
        assert_eq!(
            data.operation(
                &mut heap,
                DataOperation::BytesConcatAll,
                0,
                &[3],
                &[ManagedSlot::Reference(wrong)]
            ),
            Err(DataError::WrongKind)
        );
        let (bytes, reference) = tagged_bytes(&mut heap, &mut data, b"A");
        let list = data
            .create_composite(&mut heap, 1, None, &[ManagedSlot::Reference(bytes)])
            .unwrap();
        assert_eq!(
            data.operation(
                &mut heap,
                DataOperation::BytesConcatAll,
                2,
                &[1],
                &[ManagedSlot::Reference(list)]
            ),
            Err(DataError::WrongKind)
        );
        heap.write_field(reference, 1, HeapValue::Null).unwrap();
        assert_eq!(
            data.operation(
                &mut heap,
                DataOperation::BytesConcatAll,
                0,
                &[1],
                &[ManagedSlot::Reference(list)]
            ),
            Err(DataError::InvalidEncoding)
        );
    }

    #[test]
    fn boolean_byte_predicates_match_search_without_a_managed_result() {
        let (mut heap, mut data) = setup();
        let alphabet: Vec<u8> = (0..=255).collect();
        for byte in 0..=255 {
            for members in [vec![], vec![byte], vec![byte, byte], alphabet.clone()] {
                for bytes in [vec![], vec![byte], alphabet.clone()] {
                    let input = data
                        .create_bytes(&mut heap, bytes.len() as u64, &bytes)
                        .unwrap();
                    let set = data
                        .create_bytes(&mut heap, members.len() as u64, &members)
                        .unwrap();
                    for (op, expected) in [
                        (
                            DataOperation::BytesContainsAny,
                            bytes.iter().any(|b| members.contains(b)),
                        ),
                        (
                            DataOperation::BytesAllIn,
                            bytes.iter().all(|b| members.contains(b)),
                        ),
                    ] {
                        let value = data
                            .operation(
                                &mut heap,
                                op,
                                7,
                                &[1, 1],
                                &[ManagedSlot::Reference(input), ManagedSlot::Reference(set)],
                            )
                            .unwrap();
                        assert_eq!(
                            value,
                            ManagedSlot::Scalar(
                                ScalarValue::from_bits(ScalarKind::Bool, u64::from(expected))
                                    .unwrap()
                            )
                        );
                    }
                    data.release(&mut heap, input).unwrap();
                    data.release(&mut heap, set).unwrap();
                }
            }
        }
    }

    #[test]
    fn predicates_validate_both_payloads_before_empty_or_early_results() {
        let (mut heap, mut data) = setup();
        let (input, reference) = tagged_bytes(&mut heap, &mut data, b"AAAAAAAAA");
        let empty = data.create_bytes(&mut heap, 0, b"").unwrap();
        heap.write_field(reference, 2, HeapValue::Null).unwrap();
        for slots in [
            [ManagedSlot::Reference(input), ManagedSlot::Reference(empty)],
            [ManagedSlot::Reference(empty), ManagedSlot::Reference(input)],
        ] {
            for op in [DataOperation::BytesContainsAny, DataOperation::BytesAllIn] {
                assert!(data.operation(&mut heap, op, 7, &[1, 1], &slots).is_err());
            }
        }
    }

    #[test]
    fn trim_matches_slice_boundaries_and_survives_input_release_and_gc() {
        let (mut heap, mut data) = setup();
        let alphabet: Vec<u8> = (0..=255).collect();
        for bytes in [
            vec![],
            vec![0],
            vec![0, 255, 128, 0],
            alphabet.clone(),
            vec![32; 257],
        ] {
            for members in [vec![], vec![0], vec![32], vec![0, 0, 255], alphabet.clone()] {
                let mut start = 0;
                let mut end = bytes.len();
                while start < end && members.contains(&bytes[start]) {
                    start += 1;
                }
                while end > start && members.contains(&bytes[end - 1]) {
                    end -= 1;
                }
                let input = data
                    .create_bytes(&mut heap, bytes.len() as u64, &bytes)
                    .unwrap();
                let original = data.any_reference(&heap, input).unwrap().0;
                let set = data
                    .create_bytes(&mut heap, members.len() as u64, &members)
                    .unwrap();
                let output = data
                    .operation(
                        &mut heap,
                        DataOperation::BytesTrim,
                        1,
                        &[1, 1],
                        &[ManagedSlot::Reference(input), ManagedSlot::Reference(set)],
                    )
                    .unwrap();
                let ManagedSlot::Reference(result) = output else {
                    panic!()
                };
                if start == 0 && end == bytes.len() {
                    assert_eq!(data.any_reference(&heap, result).unwrap().0, original);
                }
                let copy = data
                    .operation(&mut heap, DataOperation::BytesCopy, 1, &[1], &[output])
                    .unwrap();
                let ManagedSlot::Reference(copied) = copy else {
                    panic!()
                };
                assert_ne!(
                    data.any_reference(&heap, result).unwrap().0,
                    data.any_reference(&heap, copied).unwrap().0
                );
                data.release(&mut heap, input).unwrap();
                data.release(&mut heap, set).unwrap();
                heap.collect(crate::heap::CollectionKind::Full).unwrap();
                for value in [output, copy] {
                    assert!(
                        matches!(data.snapshot(&heap, 1, value).unwrap(), Value::Bytes(actual) if actual == bytes[start..end])
                    );
                }
                data.release(&mut heap, result).unwrap();
                data.release(&mut heap, copied).unwrap();
                heap.collect(crate::heap::CollectionKind::Full).unwrap();
                assert_eq!(heap.object_count(), 0);
            }
        }
    }

    #[test]
    fn trim_validates_both_payloads_and_domain_before_noop_or_empty_results() {
        let (mut heap, mut data) = setup();
        let (malformed, reference) = tagged_bytes(&mut heap, &mut data, b"AAAAAAAAA");
        let empty = data.create_bytes(&mut heap, 0, b"").unwrap();
        heap.write_field(reference, 2, HeapValue::Null).unwrap();
        for slots in [
            [
                ManagedSlot::Reference(malformed),
                ManagedSlot::Reference(empty),
            ],
            [
                ManagedSlot::Reference(empty),
                ManagedSlot::Reference(malformed),
            ],
        ] {
            assert!(
                data.operation(&mut heap, DataOperation::BytesTrim, 1, &[1, 1], &slots)
                    .is_err()
            );
        }
        let mut foreign = DomainHeap::new(999);
        assert!(
            data.operation(
                &mut foreign,
                DataOperation::BytesTrim,
                1,
                &[1, 1],
                &[ManagedSlot::Reference(empty), ManagedSlot::Reference(empty)]
            )
            .is_err()
        );
    }

    #[test]
    fn reads_reject_bad_tail_fields_and_padding_before_returning_a_hit_or_miss() {
        let (mut heap, mut data) = setup();
        let (input, reference) = tagged_bytes(&mut heap, &mut data, b"AAAAAAAAA");
        let set = data.create_bytes(&mut heap, 1, b"A").unwrap();
        for bad in [HeapValue::Null, HeapValue::Integer(0x141)] {
            heap.write_field(reference, 2, bad).unwrap();
            for start in [0, -1, i64::MAX] {
                assert!(
                    data.operation(
                        &mut heap,
                        DataOperation::BytesGet,
                        4,
                        &[1, 0],
                        &[ManagedSlot::Reference(input), index(start)]
                    )
                    .is_err()
                );
                for op in [DataOperation::BytesFindAny, DataOperation::BytesFindNotAny] {
                    assert!(
                        data.operation(
                            &mut heap,
                            op,
                            2,
                            &[1, 1, 0],
                            &[
                                ManagedSlot::Reference(input),
                                ManagedSlot::Reference(set),
                                index(start)
                            ]
                        )
                        .is_err()
                    );
                }
                assert!(
                    data.operation(
                        &mut heap,
                        DataOperation::BytesSlice,
                        5,
                        &[1, 0, 0],
                        &[ManagedSlot::Reference(input), index(0), index(start)]
                    )
                    .is_err()
                );
                assert_eq!(data.root_count(), 2);
            }
        }
        heap.write_field(reference, 2, HeapValue::Integer(0x41))
            .unwrap();
        let set_ref = data.reference(&heap, set, DataKind::Bytes(1)).unwrap();
        heap.write_integer_fields(set_ref, 1, [0x141].into_iter())
            .unwrap();
        assert!(
            data.operation(
                &mut heap,
                DataOperation::BytesFindAny,
                2,
                &[1, 1, 0],
                &[
                    ManagedSlot::Reference(input),
                    ManagedSlot::Reference(set),
                    index(-1)
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn packed_scans_match_slice_search_at_every_word_and_payload_boundary() {
        let (mut heap, mut data) = setup();
        let sets = [
            Vec::new(),
            (0..=255).collect(),
            (0..=255).step_by(2).collect(),
        ];
        for length in 0..26 {
            let bytes: Vec<_> = (0..length).map(|n| (n * 37) as u8).collect();
            let input = data.create_bytes(&mut heap, length as u64, &bytes).unwrap();
            for set in &sets {
                let set_handle = data.create_bytes(&mut heap, set.len() as u64, set).unwrap();
                for start in 0..=length {
                    for op in [DataOperation::BytesFindAny, DataOperation::BytesFindNotAny] {
                        let expected = bytes[start..]
                            .iter()
                            .position(|byte| {
                                set.contains(byte) == (op == DataOperation::BytesFindAny)
                            })
                            .map(|offset| (start + offset) as u64);
                        let output = data
                            .operation(
                                &mut heap,
                                op,
                                2,
                                &[1, 1, 0],
                                &[
                                    ManagedSlot::Reference(input),
                                    ManagedSlot::Reference(set_handle),
                                    index(start as i64),
                                ],
                            )
                            .unwrap();
                        heap.collect(crate::heap::CollectionKind::Full).unwrap();
                        let Value::Enum {
                            variant, fields, ..
                        } = data.snapshot(&heap, 2, output).unwrap()
                        else {
                            panic!()
                        };
                        assert_eq!(variant, u32::from(expected.is_some()));
                        if let Some(expected) = expected {
                            assert!(
                                matches!(fields[0], Value::Scalar(value) if value.bits() == expected)
                            );
                        }
                        let ManagedSlot::Reference(handle) = output else {
                            panic!()
                        };
                        data.release(&mut heap, handle).unwrap();
                    }
                }
                data.release(&mut heap, set_handle).unwrap();
            }
            data.release(&mut heap, input).unwrap();
        }
        heap.collect(crate::heap::CollectionKind::Full).unwrap();
        assert_eq!((data.root_count(), heap.object_count()), (0, 0));
    }

    #[test]
    fn byte_array_rejects_a_late_invalid_u8_before_publishing_a_result() {
        let (mut heap, mut data) = setup();
        let scalar = |n| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::U8, n).unwrap());
        let list = data
            .create_composite(&mut heap, 6, None, &[scalar(1), scalar(2)])
            .unwrap();
        let reference = data.reference(&heap, list, DataKind::Composite(6)).unwrap();
        heap.write_field(reference, 3, HeapValue::Integer(256))
            .unwrap();
        assert!(
            data.operation(
                &mut heap,
                DataOperation::ByteArray,
                1,
                &[6],
                &[ManagedSlot::Reference(list)]
            )
            .is_err()
        );
        assert_eq!((data.root_count(), heap.object_count()), (1, 1));
    }

    #[test]
    fn slices_and_byte_array_construction_preserve_values_gc_and_root_lifetimes() {
        for length in [0, 1, 7, 8, 9, 65536] {
            let (mut heap, mut data) = setup();
            let bytes: Vec<_> = (0..length).map(|n| n as u8).collect();
            let input = data.create_bytes(&mut heap, length as u64, &bytes).unwrap();
            for (start, end) in [
                (0, length as i64),
                (0, 0),
                (length as i64, length as i64),
                (-1, 0),
                (1, 0),
                (0, length as i64 + 1),
            ] {
                let output = data
                    .operation(
                        &mut heap,
                        DataOperation::BytesSlice,
                        5,
                        &[1, 0, 0],
                        &[ManagedSlot::Reference(input), index(start), index(end)],
                    )
                    .unwrap();
                assert_eq!(data.root_count(), 2);
                heap.collect(crate::heap::CollectionKind::Full).unwrap();
                let Value::Enum {
                    variant, fields, ..
                } = data.snapshot(&heap, 5, output).unwrap()
                else {
                    panic!()
                };
                let valid = start >= 0 && end >= start && end <= length as i64;
                assert_eq!(variant, u32::from(valid));
                if valid {
                    assert!(
                        matches!(&fields[0], Value::Bytes(value) if *value == bytes[start as usize..end as usize])
                    );
                }
                let ManagedSlot::Reference(handle) = output else {
                    panic!()
                };
                data.release(&mut heap, handle).unwrap();
                assert_eq!(data.root_count(), 1);
            }
            assert!(
                data.operation(
                    &mut heap,
                    DataOperation::BytesSlice,
                    0,
                    &[1, 0, 0],
                    &[ManagedSlot::Reference(input), index(0), index(0)]
                )
                .is_err()
            );
            assert_eq!(data.root_count(), 1);
            let fields: Vec<_> = bytes
                .iter()
                .map(|byte| {
                    ManagedSlot::Scalar(
                        ScalarValue::from_bits(ScalarKind::U8, u64::from(*byte)).unwrap(),
                    )
                })
                .collect();
            let list = data.create_composite(&mut heap, 6, None, &fields).unwrap();
            let output = data
                .operation(
                    &mut heap,
                    DataOperation::ByteArray,
                    1,
                    &[6],
                    &[ManagedSlot::Reference(list)],
                )
                .unwrap();
            data.release(&mut heap, input).unwrap();
            data.release(&mut heap, list).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(data.root_count(), 1);
            assert!(
                matches!(data.snapshot(&heap, 1, output).unwrap(), Value::Bytes(value) if value == bytes)
            );
            let ManagedSlot::Reference(handle) = output else {
                panic!()
            };
            data.release(&mut heap, handle).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!(heap.object_count(), 0);
        }
    }

    #[test]
    fn fixed_byte_array_alias_outlives_input_root_and_explicit_copy_stays_independent() {
        for length in [0, 1, 7, 8, 9, 65536] {
            let mut heap = DomainHeap::new(315);
            let mut data = ManagedData::new(315);
            data.configure_values(
                &heap,
                Arc::new(
                    ValueSchema::new(vec![
                        ValueType::Bytes(length),
                        ValueType::DynamicBytes,
                        ValueType::Scalar(ScalarKind::I64),
                        ValueType::Bytes(length + 1),
                    ])
                    .unwrap(),
                ),
            )
            .unwrap();
            let bytes: Vec<_> = (0..length).map(|n| n as u8).collect();
            let input = data.create_bytes(&mut heap, length, &bytes).unwrap();
            for (result, ty) in [(2, 0), (1, 3)] {
                assert!(
                    data.operation(
                        &mut heap,
                        DataOperation::ByteArray,
                        result,
                        &[ty],
                        &[ManagedSlot::Reference(input)]
                    )
                    .is_err()
                );
                assert_eq!((heap.object_count(), data.root_count()), (1, 1));
            }
            let output = data
                .operation(
                    &mut heap,
                    DataOperation::ByteArray,
                    1,
                    &[0],
                    &[ManagedSlot::Reference(input)],
                )
                .unwrap();
            let ManagedSlot::Reference(output) = output else {
                panic!()
            };
            assert_ne!(input, output);
            assert_eq!(
                data.reference(&heap, input, DataKind::Bytes(length))
                    .unwrap(),
                data.reference(&heap, output, DataKind::Bytes(length))
                    .unwrap()
            );
            assert_eq!((heap.object_count(), data.root_count()), (1, 2));
            data.release(&mut heap, input).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert!(
                matches!(data.snapshot(&heap, 1, ManagedSlot::Reference(output)).unwrap(),
                Value::Bytes(actual) if actual == bytes)
            );
            let copied = data
                .operation(
                    &mut heap,
                    DataOperation::BytesCopy,
                    1,
                    &[1],
                    &[ManagedSlot::Reference(output)],
                )
                .unwrap();
            let ManagedSlot::Reference(copied) = copied else {
                panic!()
            };
            assert_ne!(
                data.reference(&heap, copied, DataKind::Bytes(length))
                    .unwrap(),
                data.reference(&heap, output, DataKind::Bytes(length))
                    .unwrap()
            );
            data.release(&mut heap, output).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!((heap.object_count(), data.root_count()), (1, 1));
            assert!(
                matches!(data.snapshot(&heap, 1, ManagedSlot::Reference(copied)).unwrap(),
                Value::Bytes(actual) if actual == bytes)
            );
            data.release(&mut heap, copied).unwrap();
            heap.collect(crate::heap::CollectionKind::Full).unwrap();
            assert_eq!((heap.object_count(), data.root_count()), (0, 0));
        }
    }

    #[test]
    fn byte_array_sharing_validates_tail_encoding_and_domain_before_new_root() {
        let mut heap = DomainHeap::new(316);
        let mut data = ManagedData::new(316);
        data.configure_values(
            &heap,
            Arc::new(ValueSchema::new(vec![ValueType::Bytes(9), ValueType::DynamicBytes]).unwrap()),
        )
        .unwrap();
        let input = data.create_bytes(&mut heap, 9, &[0; 9]).unwrap();
        let mut foreign = DomainHeap::new(317);
        assert!(
            data.operation(
                &mut foreign,
                DataOperation::ByteArray,
                1,
                &[0],
                &[ManagedSlot::Reference(input)]
            )
            .is_err()
        );
        let reference = data.reference(&heap, input, DataKind::Bytes(9)).unwrap();
        heap.write_integer_fields(reference, 2, [256].into_iter())
            .unwrap();
        assert!(
            data.operation(
                &mut heap,
                DataOperation::ByteArray,
                1,
                &[0],
                &[ManagedSlot::Reference(input)]
            )
            .is_err()
        );
        assert_eq!((heap.object_count(), data.root_count()), (1, 1));
    }

    #[test]
    fn byte_scans_cover_the_full_alphabet_boundaries_and_immutable_ascii_case() {
        let mut heap = DomainHeap::new(311);
        let mut data = ManagedData::new(311);
        let schema = ValueSchema::new(vec![
            ValueType::Scalar(ScalarKind::I64),
            ValueType::DynamicBytes,
            ValueType::Enum {
                identity: "Option<Int>".into(),
                variants: vec![vec![], vec![0]],
            },
        ])
        .unwrap();
        data.configure_values(&heap, Arc::new(schema)).unwrap();
        let bytes: Vec<_> = (0..=255).collect();
        let input = ManagedSlot::Reference(data.create_bytes(&mut heap, 256, &bytes).unwrap());
        for (op, set, start, expected) in [
            (DataOperation::BytesFindAny, vec![0, 255], 1, Some(255)),
            (DataOperation::BytesFindAny, vec![128], 128, Some(128)),
            (DataOperation::BytesFindAny, vec![], 0, None),
            (DataOperation::BytesFindAny, bytes.clone(), 256, None),
            (DataOperation::BytesFindAny, bytes.clone(), -1, None),
            (DataOperation::BytesFindAny, bytes.clone(), i64::MAX, None),
            (DataOperation::BytesFindNotAny, vec![], 255, Some(255)),
            (DataOperation::BytesFindNotAny, bytes.clone(), 0, None),
            (DataOperation::BytesFindNotAny, vec![0], 0, Some(1)),
        ] {
            let set = ManagedSlot::Reference(
                data.create_bytes(&mut heap, set.len() as u64, &set)
                    .unwrap(),
            );
            let start =
                ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, start as u64).unwrap());
            let result = data
                .operation(&mut heap, op, 2, &[1, 1, 0], &[input, set, start])
                .unwrap();
            let Value::Enum {
                variant, fields, ..
            } = data.snapshot(&heap, 2, result).unwrap()
            else {
                panic!()
            };
            assert_eq!(variant, u32::from(expected.is_some()));
            if let Some(expected) = expected {
                assert!(
                    matches!(fields.as_slice(), [Value::Scalar(value)] if value.bits() == expected)
                );
            }
        }
        let lower = data
            .operation(&mut heap, DataOperation::BytesLowerAscii, 1, &[1], &[input])
            .unwrap();
        assert_ne!(lower, input);
        heap.collect(crate::heap::CollectionKind::Full).unwrap();
        assert!(
            matches!(data.snapshot(&heap, 1, input).unwrap(), Value::Bytes(value) if value == bytes)
        );
        let mut expected = bytes;
        expected.make_ascii_lowercase();
        assert!(
            matches!(data.snapshot(&heap, 1, lower).unwrap(), Value::Bytes(value) if value == expected)
        );
    }
}
