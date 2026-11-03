use rinzu_runtime::{
    data::ManagedData,
    error_value::ErrorValue,
    failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition},
    heap::{CollectionKind, DomainHeap},
    transport::{TransportBudget, TransportDirection, typed::Descriptor},
    value::{Value, ValueSchema, ValueType},
};
use std::sync::Arc;
#[test]
fn errors_transfer_without_equality_and_preserve_diagnostics() {
    let schema = Arc::new(ValueSchema::new(vec![ValueType::Error, ValueType::List(0)]).unwrap());
    assert!(!schema.has_equality(0).unwrap());
    assert!(!schema.has_equality(1).unwrap());
    assert!(schema.has_transferable(1).unwrap());
    let descriptor = Descriptor::new(schema.clone(), vec![1]).unwrap();
    let original = ErrorValue::new(1200, "日本語\0tail")
        .with_context("file".into())
        .with_cause(&ErrorValue::new(1201, "root"))
        .unwrap();
    let values = [Value::List(vec![Value::Error(original.clone())])];
    let exact = descriptor.preflight(&values).unwrap();
    let budget = TransportBudget::new(4096, 4096);
    let packet = descriptor
        .encode(
            &values,
            budget.reserve(TransportDirection::Input, exact).unwrap(),
        )
        .unwrap();
    for i in 0..exact {
        assert!(descriptor.decode(&packet.bytes()[..i]).is_err());
    }
    let decoded = packet.decode_typed(&descriptor).unwrap();
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    data.configure_values(&heap, schema).unwrap();
    let slots = data.materialize_values(&mut heap, &[1], &decoded).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let Value::List(copied) = data.snapshot(&heap, 1, slots[0]).unwrap() else {
        panic!()
    };
    let Value::Error(copied) = &copied[0] else {
        panic!()
    };
    assert_eq!(&original, copied);
    let failure = Failure {
        managed: Default::default(),
        traces: Default::default(),
        primary: Diagnostic {
            cause: FailureCause::Error(ErrorKind::Conversion),
            source: Some(SourcePosition {
                file: u64::MAX,
                start_byte: 3,
                end_byte: 5,
            }),
        },
        cleanup: vec![
            FailureCause::Error(ErrorKind::Io).into(),
            FailureCause::Error(ErrorKind::NativeRecordBusy).into(),
        ],
    };
    let caught = data.create_error(&mut heap, &failure).unwrap();
    let value = data.error_value(&heap, caught).unwrap();
    assert_eq!(value.root().code, 1);
    assert_eq!(value.root().source.unwrap().file, u64::MAX);
    assert_eq!(
        value
            .root()
            .cleanup
            .iter()
            .map(|i| value.nodes[*i].code)
            .collect::<Vec<_>>(),
        [6, 4]
    );
    let before = heap.object_count();
    let mut bad = original.clone();
    bad.nodes[0].cause = Some(0);
    assert!(!bad.validate());
    assert!(
        data.materialize_values(&mut heap, &[0], &[Value::Error(bad)])
            .is_err()
    );
    assert_eq!(heap.object_count(), before);
    let mut large = ErrorValue::new(1000, "start");
    for _ in 1..64 {
        large = ErrorValue::new(1001, "outer").with_cause(&large).unwrap();
    }
    assert_eq!(large.nodes.len(), 64);
    assert_eq!(
        ErrorValue::new(1002, "too large")
            .with_cause(&large)
            .unwrap_err()
            .root()
            .code,
        101
    );
    let bytes = original.encode();
    for i in 0..bytes.len() {
        assert!(ErrorValue::decode(&bytes[..i]).is_none());
    }
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    drop(packet);
    assert_eq!(
        (
            heap.object_count(),
            data.root_count(),
            budget.snapshot().used_bytes
        ),
        (0, 0, 0)
    );
}
