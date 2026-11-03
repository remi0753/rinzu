use rinzu_runtime::{
    data::ManagedData,
    heap::{CollectionKind, DomainHeap},
    transport::{TransportBudget, TransportDirection, typed::Descriptor},
    value::{Value, ValueSchema, ValueType},
};
use std::sync::Arc;
#[test]
fn dynamic_binary_packets_validate_lengths_and_preserve_all_bytes() {
    let schema =
        Arc::new(ValueSchema::new(vec![ValueType::DynamicBytes, ValueType::Bytes(3)]).unwrap());
    let descriptor = Descriptor::new(schema.clone(), vec![0]).unwrap();
    let budget = TransportBudget::new(1024, 1024);
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    data.configure_values(&heap, schema).unwrap();
    for bytes in [
        vec![],
        vec![0],
        vec![0, 255, 128],
        "日本語\0e\u{301}".as_bytes().to_vec(),
    ] {
        let values = [Value::Bytes(bytes)];
        let exact = descriptor.preflight(&values).unwrap();
        let packet = descriptor
            .encode(
                &values,
                budget.reserve(TransportDirection::Input, exact).unwrap(),
            )
            .unwrap();
        assert_eq!(packet.bytes().len(), exact);
        for n in 0..exact {
            assert!(descriptor.decode(&packet.bytes()[..n]).is_err());
        }
        let decoded = packet.decode_typed(&descriptor).unwrap();
        let slots = data.materialize_values(&mut heap, &[0], &decoded).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert!(data.values_equal(&heap, 0, slots[0], slots[0]).unwrap());
        data.release_all(&mut heap).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
    }
    assert!(descriptor.decode(&u64::MAX.to_le_bytes()).is_err());
    assert_eq!(
        (
            heap.object_count(),
            data.root_count(),
            budget.snapshot().used_bytes
        ),
        (0, 0, 0)
    );
}
