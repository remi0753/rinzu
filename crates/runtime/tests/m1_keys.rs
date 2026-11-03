use rinzu_runtime::{
    data::ManagedData,
    heap::{CollectionKind, DomainHeap},
    scalar::{ScalarKind, ScalarValue},
    transport::{TransportBudget, TransportDirection, typed::Descriptor},
    value::{Value, ValueError, ValueSchema, ValueType},
};
use std::sync::Arc;

fn boolean(value: bool) -> Value {
    Value::Scalar(ScalarValue::from_bool(value))
}
fn record(flag: bool) -> Value {
    Value::Record {
        identity: "Outer".into(),
        fields: vec![
            Value::Record {
                identity: "Inner".into(),
                fields: vec![boolean(true), Value::Bytes(vec![0, 255])],
            },
            Value::Bytes(vec![65, 0, 0, 0]),
            boolean(flag),
        ],
    }
}

#[test]
fn record_key_maps_validate_packets_and_retain_hash_across_worker_gc() {
    let schema = Arc::new(
        ValueSchema::new(vec![
            ValueType::Scalar(ScalarKind::Bool),
            ValueType::DynamicBytes,
            ValueType::Bytes(4),
            ValueType::Record {
                identity: "Inner".into(),
                fields: vec![0, 1],
            },
            ValueType::Record {
                identity: "Outer".into(),
                fields: vec![3, 2, 0],
            },
            ValueType::Map { key: 4, value: 0 },
        ])
        .unwrap(),
    );
    let original = record(false);
    let other = record(true);
    assert_eq!(schema.key_hash(4, &original).unwrap(), 1795943501115571834);
    let value = schema
        .map(
            5,
            vec![
                (original.clone(), boolean(false)),
                (other.clone(), boolean(false)),
                (original.clone(), boolean(true)),
            ],
        )
        .unwrap();
    let Value::Map(entries) = &value else {
        panic!()
    };
    assert_eq!(entries.len(), 2);
    assert!(schema.equal(4, &entries[0].0, &original).unwrap());
    assert!(schema.equal(0, &entries[0].1, &boolean(true)).unwrap());
    let duplicate = Value::Map(vec![
        (original.clone(), boolean(true)),
        (original, boolean(false)),
    ]);
    assert_eq!(
        schema.validate(5, &duplicate),
        Err(ValueError::DuplicateKey)
    );
    let descriptor = Descriptor::new(schema.clone(), vec![5]).unwrap();
    assert!(descriptor.preflight(&[duplicate]).is_err());
    let budget = TransportBudget::new(4096, 4096);
    let exact = descriptor.preflight(std::slice::from_ref(&value)).unwrap();
    let packet = descriptor
        .encode(
            std::slice::from_ref(&value),
            budget.reserve(TransportDirection::Input, exact).unwrap(),
        )
        .unwrap();
    for end in 0..exact {
        assert!(descriptor.decode(&packet.bytes()[..end]).is_err());
    }
    let hash = schema.key_hash(4, &entries[0].0).unwrap();
    let worker_schema = schema.clone();
    let copied = std::thread::spawn(move || {
        let values = packet.decode_typed(&descriptor).unwrap();
        let mut heap = DomainHeap::new(2);
        let mut data = ManagedData::new(2);
        data.configure_values(&heap, worker_schema.clone()).unwrap();
        let slots = data.materialize_values(&mut heap, &[5], &values).unwrap();
        for _ in 0..8 {
            heap.collect(CollectionKind::Full).unwrap();
        }
        let snapshot = data.snapshot(&heap, 5, slots[0]).unwrap();
        let Value::Map(entries) = &snapshot else {
            panic!()
        };
        assert_eq!(worker_schema.key_hash(4, &entries[0].0).unwrap(), hash);
        data.release_all(&mut heap).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!((heap.object_count(), data.root_count()), (0, 0));
        snapshot
    })
    .join()
    .unwrap();
    assert!(schema.equal(5, &value, &copied).unwrap());
    assert_eq!(budget.snapshot().used_bytes, 0);
}
