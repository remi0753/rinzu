use rinzu_runtime::data::{ManagedData, ManagedSlot};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as S};
use rinzu_runtime::transport::{
    TransportBudget, TransportDirection as D, TransportError as E, typed::Descriptor,
};
use rinzu_runtime::value::{Value as V, ValueSchema, ValueType as T};
use std::sync::Arc;

fn scalar(kind: K, bits: u64) -> V {
    V::Scalar(S::from_bits(kind, bits).unwrap())
}
fn schema() -> Arc<ValueSchema> {
    Arc::new(
        ValueSchema::new(vec![
            T::Scalar(K::U64),
            T::Scalar(K::F64),
            T::String,
            T::Bytes(3),
            T::Record {
                identity: "fixture::Packet".into(),
                fields: vec![0, 1, 2, 3],
            },
            T::Enum {
                identity: "fixture::Key".into(),
                variants: vec![vec![0, 2], vec![]],
            },
            T::List(4),
            T::Map { key: 5, value: 6 },
            T::Unit,
            T::Other,
            T::Enum {
                identity: "fixture::NotTransferable".into(),
                variants: vec![vec![], vec![9]],
            },
            T::List(10),
            T::Bytes(0),
        ])
        .unwrap(),
    )
}
fn packet(bits: u64) -> V {
    V::Record {
        identity: "fixture::Packet".into(),
        fields: vec![
            scalar(K::U64, u64::MAX),
            scalar(K::F64, bits),
            V::String("日本語\0🙂".into()),
            V::Bytes(vec![0, 255, 42]),
        ],
    }
}
fn values() -> Vec<V> {
    vec![
        V::Map(vec![
            (
                V::Enum {
                    identity: "fixture::Key".into(),
                    variant: 0,
                    fields: vec![scalar(K::U64, 1 << 63), V::String("a\0b".into())],
                },
                V::List(vec![
                    packet(f64::NAN.to_bits()),
                    packet((-0.0f64).to_bits()),
                ]),
            ),
            (
                V::Enum {
                    identity: "fixture::Key".into(),
                    variant: 1,
                    fields: vec![],
                },
                V::List(vec![]),
            ),
        ]),
        V::Unit,
        V::Bytes(vec![]),
    ]
}

#[test]
fn receivers_copy_the_complete_vector_to_distinct_heaps_before_credit_settlement() {
    let schema = schema();
    let roots = [7, 8, 12];
    let descriptor = Descriptor::new(schema.clone(), roots.to_vec()).unwrap();
    let values = values();
    let exact = descriptor.preflight(&values).unwrap();
    let budget = TransportBudget::new(4096, 4096);
    let buffer = descriptor
        .encode(&values, budget.reserve(D::Input, exact).unwrap())
        .unwrap();
    let decoded = buffer.decode_typed(&descriptor).unwrap();
    let mut heap = DomainHeap::new(991);
    let mut data = ManagedData::new(991);
    data.configure_values(&heap, schema.clone()).unwrap();
    let mut other_heap = DomainHeap::new(992);
    let mut other = ManagedData::new(992);
    other.configure_values(&other_heap, schema).unwrap();
    let slots = data
        .materialize_values(&mut heap, &roots, &decoded)
        .unwrap();
    let other_slots = other
        .materialize_values(&mut other_heap, &roots, &decoded)
        .unwrap();
    assert_ne!(slots[0], other_slots[0]);
    assert!(other.snapshot(&other_heap, roots[0], slots[0]).is_err());
    assert_eq!(budget.snapshot().used_bytes, exact);
    drop(decoded);
    drop(buffer);
    assert_eq!(budget.snapshot().used_bytes, 0);
    let live = slots
        .iter()
        .filter_map(|slot| {
            if let ManagedSlot::Reference(handle) = slot {
                Some(*handle)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    data.retain_roots(&mut heap, live).unwrap();
    for _ in 0..4 {
        assert!(heap.collect(CollectionKind::Full).unwrap().moved_objects > 5);
        let snapshot = roots
            .iter()
            .zip(&slots)
            .map(|(&ty, &slot)| data.snapshot(&heap, ty, slot).unwrap())
            .collect::<Vec<_>>();
        let copy = descriptor
            .encode(&snapshot, budget.reserve(D::Result, exact).unwrap())
            .unwrap();
        assert_eq!(
            copy.bytes(),
            descriptor
                .encode(&values, budget.reserve(D::Input, exact).unwrap())
                .unwrap()
                .bytes()
        );
    }
    let before = (heap.object_count(), data.root_count());
    let mut bad = values;
    bad[2] = V::Bytes(vec![0]);
    assert!(data.materialize_values(&mut heap, &roots, &bad).is_err());
    assert!(
        data.materialize_values(
            &mut heap,
            &[10],
            &[V::Enum {
                identity: "fixture::NotTransferable".into(),
                variant: 0,
                fields: vec![]
            }]
        )
        .is_err()
    );
    assert_eq!((heap.object_count(), data.root_count()), before);
    data.release_all(&mut heap).unwrap();
    other.release_all(&mut other_heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    other_heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        (
            heap.object_count(),
            data.root_count(),
            other_heap.object_count(),
            other.root_count(),
            budget.snapshot().used_bytes
        ),
        (0, 0, 0, 0, 0)
    );
}
#[test]
fn typed_packets_preserve_nominal_lengths_ieee_and_exact_credit_until_receiver_copy() {
    let schema = schema();
    let descriptor = Descriptor::new(schema, vec![7, 8, 12]).unwrap();
    let values = values();
    let exact = descriptor.preflight(&values).unwrap();
    let budget = TransportBudget::new(1024, 1024);
    budget.enable_evidence();
    let buffer = descriptor
        .encode(&values, budget.reserve(D::Input, exact).unwrap())
        .unwrap();
    assert_eq!(buffer.bytes().len(), exact);
    let copy = buffer.decode_typed(&descriptor).unwrap();
    assert_eq!(budget.snapshot().used_bytes, exact);
    let encoded_copy = descriptor
        .encode(&copy, budget.reserve(D::Result, exact).unwrap())
        .unwrap();
    assert_eq!(buffer.bytes(), encoded_copy.bytes());
    drop(buffer);
    drop(encoded_copy);
    assert_eq!(budget.snapshot().used_bytes, 0);
    assert!(
        budget
            .evidence()
            .iter()
            .all(|credit| credit.released_at.is_some())
    );
    assert!(matches!(
        descriptor.encode(&values, budget.reserve(D::Input, exact - 1).unwrap()),
        Err(E::ReservationSize { .. })
    ));
    assert_eq!(budget.snapshot().used_bytes, 0);
}
#[test]
fn whole_schema_and_values_reject_unused_payloads_and_late_corruption_without_partial_output() {
    let schema = schema();
    assert!(matches!(
        Descriptor::new(schema.clone(), vec![11]),
        Err(E::NonTransferable(9))
    ));
    let descriptor = Descriptor::new(schema.clone(), vec![4]).unwrap();
    let budget = TransportBudget::new(1024, 1024);
    let good = vec![packet(0)];
    let exact = descriptor.preflight(&good).unwrap();
    let buffer = descriptor
        .encode(&good, budget.reserve(D::Input, exact).unwrap())
        .unwrap();
    for count in 0..buffer.bytes().len() {
        assert!(descriptor.decode(&buffer.bytes()[..count]).is_err());
    }
    let mut wrong_identity = buffer.bytes().to_vec();
    wrong_identity[8] = b'X';
    assert!(descriptor.decode(&wrong_identity).is_err());
    let mut wrong_length = buffer.bytes().to_vec();
    let start = wrong_length.len() - 3 - 8;
    wrong_length[start] = 2;
    assert!(descriptor.decode(&wrong_length).is_err());
    let mut invalid_utf8 = buffer.bytes().to_vec();
    invalid_utf8[start - 1] = 255;
    assert!(matches!(
        descriptor.decode(&invalid_utf8),
        Err(E::InvalidUtf8)
    ));
    let mut trailing = buffer.bytes().to_vec();
    trailing.push(0);
    assert_eq!(
        descriptor.decode(&trailing).unwrap_err(),
        E::TrailingBytes(1)
    );
    drop(buffer);
    let mut bad = good;
    let V::Record { fields, .. } = &mut bad[0] else {
        panic!()
    };
    fields[3] = V::Bytes(vec![0, 255]);
    assert!(
        descriptor
            .encode(&bad, budget.reserve(D::Input, exact).unwrap())
            .is_err()
    );
    assert_eq!(budget.snapshot().used_bytes, 0);
    let list = Descriptor::new(schema.clone(), vec![6]).unwrap();
    assert!(matches!(
        list.decode(&u64::MAX.to_le_bytes()),
        Err(E::Truncated)
    ));
    let map = Descriptor::new(schema, vec![7]).unwrap();
    assert!(matches!(
        map.decode(&u64::MAX.to_le_bytes()),
        Err(E::Truncated)
    ));
    let cycle = Arc::new(ValueSchema::new(vec![T::List(0)]).unwrap());
    assert!(matches!(
        Descriptor::new(cycle, vec![0]),
        Err(E::NonTransferable(0))
    ));
}
#[test]
fn indexed_walks_support_deep_graphs_and_all_scalar_widths_without_address_encoding() {
    let mut types = vec![T::String];
    for index in 0..512 {
        types.push(T::List(index));
    }
    let schema = Arc::new(ValueSchema::new(types).unwrap());
    let descriptor = Descriptor::new(schema.clone(), vec![512]).unwrap();
    let mut value = V::String("deep\0".into());
    for _ in 0..512 {
        value = V::List(vec![value]);
    }
    let budget = TransportBudget::new(1 << 20, 1 << 20);
    let values = vec![value];
    let exact = descriptor.preflight(&values).unwrap();
    assert_eq!(exact, 512 * 8 + 8 + 5);
    let buffer = descriptor
        .encode(&values, budget.reserve(D::Input, exact).unwrap())
        .unwrap();
    let mut heap = DomainHeap::new(513);
    let mut data = ManagedData::new(513);
    data.configure_values(&heap, schema).unwrap();
    let slots = data
        .materialize_values(
            &mut heap,
            &[512],
            &buffer.decode_typed(&descriptor).unwrap(),
        )
        .unwrap();
    let ManagedSlot::Reference(root) = slots[0] else {
        panic!("deep reference")
    };
    data.retain_roots(&mut heap, [root]).unwrap();
    assert_eq!(
        heap.collect(CollectionKind::Full).unwrap().moved_objects,
        513
    );
    let copied = data.snapshot(&heap, 512, slots[0]).unwrap();
    assert_eq!(
        descriptor
            .encode(&[copied], budget.reserve(D::Result, exact).unwrap())
            .unwrap()
            .bytes(),
        buffer.bytes()
    );
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((heap.object_count(), data.root_count()), (0, 0));
    assert_eq!(
        descriptor
            .preflight(&buffer.decode_typed(&descriptor).unwrap())
            .unwrap(),
        exact
    );
    drop(buffer);
    for kind in [
        K::I8,
        K::U8,
        K::I16,
        K::U16,
        K::I32,
        K::U32,
        K::I64,
        K::U64,
        K::F32,
        K::F64,
        K::Bool,
    ] {
        let descriptor = Descriptor::new(
            Arc::new(ValueSchema::new(vec![T::Scalar(kind)]).unwrap()),
            vec![0],
        )
        .unwrap();
        let bits = match kind {
            K::Bool => 1,
            K::F32 => u64::from(f32::NAN.to_bits()),
            K::F64 => f64::NAN.to_bits(),
            _ if kind.signed() => u64::MAX,
            _ if kind.width() < 64 => (1 << kind.width()) - 1,
            _ => u64::MAX,
        };
        let values = vec![scalar(kind, bits)];
        let exact = descriptor.preflight(&values).unwrap();
        assert_eq!(exact, kind.width() as usize / 8);
        let buffer = descriptor
            .encode(&values, budget.reserve(D::Input, exact).unwrap())
            .unwrap();
        let copy = buffer.materialize_typed(&descriptor).unwrap();
        let V::Scalar(copy) = copy[0] else { panic!() };
        assert_eq!(copy.kind(), kind);
        assert_eq!(copy.bits(), bits);
    }
    assert_eq!(budget.snapshot().used_bytes, 0);
}

#[test]
fn admitted_inputs_hold_credit_through_preparation_and_gate_results_until_commit() {
    use rinzu_runtime::spawn::{OriginDomainId, SpawnAdmissionController, SpawnAdmissionError};
    let schema = schema();
    let descriptor = Descriptor::new(schema, vec![7, 8, 12]).unwrap();
    let values = values();
    let exact = descriptor.preflight(&values).unwrap();
    let controller = SpawnAdmissionController::new();
    controller.enable_transport_evidence();
    let origin = OriginDomainId(0);
    let mut admitted = controller.admit(origin, exact).unwrap();
    let input = admitted.encode_typed_input(&descriptor, &values).unwrap();
    assert!(matches!(
        admitted.encode_typed_result(&descriptor, &values),
        Err(SpawnAdmissionError::InputNotSettled)
    ));
    let prepared = admitted
        .prepare_typed_input(&descriptor, input, |values| {
            assert_eq!(controller.snapshot(origin).transport_bytes, exact);
            Ok(values)
        })
        .unwrap();
    assert!(admitted.input_settled());
    assert_eq!(controller.snapshot(origin).transport_bytes, 0);
    let result = admitted
        .encode_typed_result(&descriptor, &prepared)
        .unwrap();
    assert_eq!(controller.snapshot(origin).transport_bytes, exact);
    assert!(result.decode_typed(&descriptor).is_ok());
    assert_eq!(controller.snapshot(origin).transport_bytes, exact);
    drop(result);
    drop(admitted);
    let end = controller.snapshot(origin);
    assert_eq!(
        (
            end.worker_domains,
            end.in_flight_for_origin,
            end.transport_bytes
        ),
        (0, 0, 0)
    );
    let intervals = controller.transport_evidence();
    assert!(intervals[0].released_at.unwrap() < intervals[1].reserved_at);
    let mut rejected = controller.admit(origin, exact).unwrap();
    let input = rejected.encode_typed_input(&descriptor, &values).unwrap();
    assert!(
        rejected
            .prepare_typed_input(&descriptor, input, |_| Err::<(), _>(E::Truncated))
            .is_err()
    );
    assert!(!rejected.input_settled());
    assert_eq!(controller.snapshot(origin).transport_bytes, 0);
    drop(rejected);
    let end = controller.snapshot(origin);
    assert_eq!(
        (
            end.worker_domains,
            end.in_flight_for_origin,
            end.transport_bytes
        ),
        (0, 0, 0)
    );
}
