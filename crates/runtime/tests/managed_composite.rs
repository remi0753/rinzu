use rinzu_runtime::data::{DataError, ManagedData, ManagedHandle, ManagedSlot as Slot};
use rinzu_runtime::failure::{ErrorKind, Failure, FailureCause};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as S};
use rinzu_runtime::value::{Value, ValueError, ValueSchema, ValueType as T};
use std::sync::Arc;

const U64: usize = 0;
const F64: usize = 1;
const STRING: usize = 3;
const BYTES: usize = 4;
const RECORD: usize = 5;
const ENUM: usize = 6;
const LIST: usize = 7;
const MAP: usize = 8;
const OTHER: usize = 9;
const ERROR_RECORD: usize = 10;

fn schema() -> Arc<ValueSchema> {
    Arc::new(
        ValueSchema::new(vec![
            T::Scalar(K::U64),
            T::Scalar(K::F64),
            T::Unit,
            T::String,
            T::Bytes(3),
            T::Record {
                identity: "fixture::Packet".into(),
                fields: vec![U64, STRING, BYTES, F64],
            },
            T::Enum {
                identity: "fixture::Result".into(),
                variants: vec![vec![RECORD], vec![RECORD], vec![]],
            },
            T::List(RECORD),
            T::Map {
                key: STRING,
                value: RECORD,
            },
            T::Other,
            T::Record {
                identity: "fixture::ErrorBox".into(),
                fields: vec![OTHER],
            },
            T::List(U64),
        ])
        .unwrap(),
    )
}
fn integer(bits: u64) -> Slot {
    Slot::Scalar(S::from_bits(K::U64, bits).unwrap())
}
fn float(bits: u64) -> Slot {
    Slot::Scalar(S::from_bits(K::F64, bits).unwrap())
}
fn reference(slot: Slot) -> ManagedHandle {
    if let Slot::Reference(handle) = slot {
        handle
    } else {
        panic!("reference expected")
    }
}
fn packet(
    data: &mut ManagedData,
    heap: &mut DomainHeap,
    integer_bits: u64,
    float_bits: u64,
) -> ManagedHandle {
    let string = data.create_string(heap, "日本語\0tail").unwrap();
    let bytes = data.create_bytes(heap, 3, &[0, 255, 42]).unwrap();
    data.create_composite(
        heap,
        RECORD,
        None,
        &[
            integer(integer_bits),
            Slot::Reference(string),
            Slot::Reference(bytes),
            float(float_bits),
        ],
    )
    .unwrap()
}

#[test]
fn composite_roots_trace_moving_payloads_and_projected_children_outlive_parents() {
    let schema = schema();
    let mut heap = DomainHeap::new(100);
    let mut data = ManagedData::new(100);
    data.configure_values(&heap, schema.clone()).unwrap();
    let record = packet(&mut data, &mut heap, u64::MAX, (-0.0f64).to_bits());
    let result = data
        .create_composite(&mut heap, ENUM, Some(0), &[Slot::Reference(record)])
        .unwrap();
    data.retain_roots(&mut heap, [result]).unwrap();
    assert_eq!(data.root_count(), 1);
    for _ in 0..4 {
        let report = heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 4);
        assert_eq!(report.moved_objects, 4);
        assert_eq!(data.enum_variant(&heap, result, ENUM).unwrap(), 0);
        let projected = reference(data.project(&mut heap, result, ENUM, Some(0), 0).unwrap());
        assert_eq!(
            data.project(&mut heap, projected, RECORD, None, 0).unwrap(),
            integer(u64::MAX)
        );
        let text = reference(data.project(&mut heap, projected, RECORD, None, 1).unwrap());
        let bytes = reference(data.project(&mut heap, projected, RECORD, None, 2).unwrap());
        assert_eq!(data.string(&heap, text).unwrap(), "日本語\0tail");
        assert_eq!(data.bytes(&heap, bytes, 3).unwrap(), [0, 255, 42]);
        assert_eq!(data.byte_length(&heap, bytes, 3).unwrap(), 3);
        assert_eq!(data.byte_length(&heap, bytes, 0), Err(DataError::WrongKind));
        data.retain_roots(&mut heap, [result]).unwrap();
    }
    let projected = reference(data.project(&mut heap, result, ENUM, Some(0), 0).unwrap());
    let before = data
        .snapshot(&heap, RECORD, Slot::Reference(projected))
        .unwrap();
    data.retain_roots(&mut heap, [projected]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 3);
    assert!(
        schema
            .equal(
                RECORD,
                &before,
                &data
                    .snapshot(&heap, RECORD, Slot::Reference(projected))
                    .unwrap()
            )
            .unwrap()
    );
    let text = reference(data.project(&mut heap, projected, RECORD, None, 1).unwrap());
    data.retain_roots(&mut heap, [text]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 1);
    assert_eq!(data.string(&heap, text).unwrap(), "日本語\0tail");
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((data.root_count(), heap.object_count()), (0, 0));
}

#[test]
fn maps_normalize_by_key_contents_and_self_equality_preserves_ieee_values() {
    let schema = schema();
    let mut heap = DomainHeap::new(101);
    let mut data = ManagedData::new(101);
    data.configure_values(&heap, schema.clone()).unwrap();
    let a = packet(&mut data, &mut heap, 1, 0.0f64.to_bits());
    let b = packet(&mut data, &mut heap, u64::MAX, (-0.0f64).to_bits());
    let c = packet(&mut data, &mut heap, u64::MAX, 0.0f64.to_bits());
    let key_a = data.create_string(&mut heap, "key\0日本語").unwrap();
    let key_b = data.create_string(&mut heap, "key\0日本語").unwrap();
    let map = data
        .create_composite(
            &mut heap,
            MAP,
            None,
            &[
                Slot::Reference(key_a),
                Slot::Reference(a),
                Slot::Reference(key_b),
                Slot::Reference(b),
            ],
        )
        .unwrap();
    let expected = data
        .create_composite(
            &mut heap,
            MAP,
            None,
            &[Slot::Reference(key_b), Slot::Reference(c)],
        )
        .unwrap();
    assert!(
        data.values_equal(&heap, MAP, Slot::Reference(map), Slot::Reference(expected))
            .unwrap()
    );
    let Value::Map(entries) = data.snapshot(&heap, MAP, Slot::Reference(map)).unwrap() else {
        panic!("Map")
    };
    assert_eq!(entries.len(), 1);
    let Value::Record { fields, .. } = &entries[0].1 else {
        panic!("Record")
    };
    assert!(matches!(fields[0], Value::Scalar(value) if value.bits() == u64::MAX));
    data.retain_roots(&mut heap, [map, expected]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert!(
        data.values_equal(&heap, MAP, Slot::Reference(map), Slot::Reference(expected))
            .unwrap()
    );
    let nan = packet(&mut data, &mut heap, u64::MAX, f64::NAN.to_bits());
    let list = data
        .create_composite(&mut heap, LIST, None, &[Slot::Reference(nan)])
        .unwrap();
    assert!(
        !data
            .values_equal(&heap, RECORD, Slot::Reference(nan), Slot::Reference(nan))
            .unwrap()
    );
    assert!(
        !data
            .values_equal(&heap, LIST, Slot::Reference(list), Slot::Reference(list))
            .unwrap()
    );
    let result = data
        .create_composite(&mut heap, ENUM, Some(1), &[Slot::Reference(nan)])
        .unwrap();
    assert!(
        !data
            .values_equal(
                &heap,
                ENUM,
                Slot::Reference(result),
                Slot::Reference(result)
            )
            .unwrap()
    );
    data.retain_roots(&mut heap, [list, result]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert!(
        !data
            .values_equal(&heap, LIST, Slot::Reference(list), Slot::Reference(list))
            .unwrap()
    );
    assert_eq!(
        data.value_hash(&heap, RECORD, Slot::Reference(result)),
        Err(ValueError::NotKey.into())
    );
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn typed_slots_reject_partial_inputs_and_do_not_trace_integer_handle_bit_patterns() {
    let mut heap = DomainHeap::new(102);
    let mut data = ManagedData::new(102);
    let schema = schema();
    data.configure_values(&heap, schema.clone()).unwrap();
    assert_eq!(
        data.configure_values(&heap, schema),
        Err(ValueError::InvalidSchema.into())
    );
    let string = data.create_string(&mut heap, "must die").unwrap();
    let raw_bits = string.bits();
    let list = data
        .create_composite(&mut heap, 11, None, &[integer(raw_bits)])
        .unwrap();
    data.retain_roots(&mut heap, [list]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 1);
    assert_eq!(
        data.project(&mut heap, list, 11, None, 0).unwrap(),
        integer(raw_bits)
    );
    assert_eq!(data.string(&heap, string), Err(DataError::UnknownHandle));
    let record = packet(&mut data, &mut heap, u64::MAX, 1.0f64.to_bits());
    let key = data.create_string(&mut heap, "key").unwrap();
    let baseline = (data.root_count(), heap.object_count());
    assert_eq!(
        data.create_composite(
            &mut heap,
            MAP,
            None,
            &[
                Slot::Reference(key),
                Slot::Reference(key),
                Slot::Reference(key),
                Slot::Reference(record)
            ]
        ),
        Err(ValueError::TypeMismatch.into())
    );
    assert_eq!(
        data.create_composite(&mut heap, ENUM, Some(99), &[Slot::Reference(record)]),
        Err(ValueError::TypeMismatch.into())
    );
    assert_eq!(
        data.create_bytes(&mut heap, 2, &[1, 2, 3]),
        Err(ValueError::TypeMismatch.into())
    );
    assert_eq!((data.root_count(), heap.object_count()), baseline);
    let result = data
        .create_composite(&mut heap, ENUM, Some(0), &[Slot::Reference(record)])
        .unwrap();
    let baseline = data.root_count();
    assert_eq!(
        data.project(&mut heap, result, ENUM, Some(1), 0),
        Err(ValueError::TypeMismatch.into())
    );
    assert_eq!(data.root_count(), baseline);
    let mut other_heap = DomainHeap::new(103);
    let mut other_data = ManagedData::new(103);
    other_data
        .configure_values(&other_heap, super_schema())
        .unwrap();
    let foreign = other_data
        .create_string(&mut other_heap, "foreign")
        .unwrap();
    assert_eq!(
        data.create_composite(&mut heap, ERROR_RECORD, None, &[Slot::Reference(foreign)]),
        Err(DataError::UnknownHandle)
    );
    assert!(matches!(
        data.snapshot(&other_heap, STRING, Slot::Reference(key)),
        Err(DataError::WrongDomain)
    ));
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn non_equality_error_fields_are_traced_and_deep_graphs_have_no_fixed_walk_capacity() {
    let mut types = vec![T::String];
    for ty in 0..512 {
        types.push(T::List(ty));
    }
    let schema = Arc::new(ValueSchema::new(types).unwrap());
    let mut heap = DomainHeap::new(104);
    let mut data = ManagedData::new(104);
    data.configure_values(&heap, schema).unwrap();
    let mut top = data.create_string(&mut heap, "deep\0").unwrap();
    for ty in 1..=512 {
        top = data
            .create_composite(&mut heap, ty, None, &[Slot::Reference(top)])
            .unwrap();
    }
    data.retain_roots(&mut heap, [top]).unwrap();
    assert_eq!(data.root_count(), 1);
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 513);
    assert!(
        data.values_equal(&heap, 512, Slot::Reference(top), Slot::Reference(top))
            .unwrap()
    );
    for ty in (1..=512).rev() {
        top = reference(data.project(&mut heap, top, ty, None, 0).unwrap());
        data.retain_roots(&mut heap, [top]).unwrap();
    }
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 1);
    assert_eq!(data.string(&heap, top).unwrap(), "deep\0");
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);

    let mut heap = DomainHeap::new(105);
    let mut data = ManagedData::new(105);
    data.configure_values(&heap, super_schema()).unwrap();
    let failure = Failure::new(FailureCause::Error(ErrorKind::InvalidUtf8));
    let error = data.create_error(&mut heap, &failure).unwrap();
    let parent = data
        .create_composite(&mut heap, ERROR_RECORD, None, &[Slot::Reference(error)])
        .unwrap();
    data.retain_roots(&mut heap, [parent]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 3);
    assert_eq!(
        data.values_equal(
            &heap,
            ERROR_RECORD,
            Slot::Reference(parent),
            Slot::Reference(parent)
        ),
        Err(ValueError::NoEquality.into())
    );
    let projected = reference(
        data.project(&mut heap, parent, ERROR_RECORD, None, 0)
            .unwrap(),
    );
    data.retain_roots(&mut heap, [projected]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.error(&heap, projected).unwrap(), failure);
    assert_eq!(heap.object_count(), 2);
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

fn super_schema() -> Arc<ValueSchema> {
    schema()
}
