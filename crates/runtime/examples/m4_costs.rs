//! Fixed-workload Rust allocation/copy probe. C arena counters are separate.
use rinzu_abi::data_operation::DataOperation;
use rinzu_runtime::data::{ManagedData, ManagedSlot};
use rinzu_runtime::heap::DomainHeap;
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::transport::{TransportBudget, TransportDirection, typed::Descriptor};
use rinzu_runtime::value::{Value, ValueSchema, ValueType};
use std::hint::black_box;
use std::sync::Arc;

#[path = "support/m4_observe.rs"]
mod observe;
use observe::measure;

fn main() {
    let iterations = std::env::args()
        .nth(1)
        .map(|value| value.parse().unwrap())
        .unwrap_or(256);
    let schema = Arc::new(
        ValueSchema::new(vec![
            ValueType::Scalar(ScalarKind::I64),
            ValueType::List(0),
            ValueType::String,
            ValueType::DynamicBytes,
            ValueType::Enum {
                identity: "Option<List<Int>>".into(),
                variants: vec![vec![], vec![1]],
            },
        ])
        .unwrap(),
    );
    let mut rows = vec![measure("scalar-inline", 8, iterations * 256, || {
        black_box(ManagedSlot::Scalar(
            ScalarValue::from_bits(ScalarKind::I64, black_box(42)).unwrap(),
        ));
    })];
    for size in [0, 8, 1024, 65536] {
        let mut heap = DomainHeap::new(501);
        let mut data = ManagedData::new(501);
        data.configure_values(&heap, schema.clone()).unwrap();
        let text = "x".repeat(size);
        let bytes = vec![7; size];
        let string = data.create_string(&mut heap, &text).unwrap();
        let binary = data.create_bytes(&mut heap, size as u64, &bytes).unwrap();
        rows.push(measure("string-length", size, iterations, || {
            black_box(data.string_length(&heap, string).unwrap());
        }));
        rows.push(measure("string-export", size, iterations, || {
            black_box(data.string(&heap, string).unwrap());
        }));
        rows.push(measure("bytes-export", size, iterations, || {
            black_box(data.bytes(&heap, binary, size as u64).unwrap());
        }));
        // A native input temporary contains the exported bytes plus terminator.
        rows.push(measure("string-native-temporary", size, iterations, || {
            let mut native = data.string(&heap, string).unwrap().into_bytes();
            native.push(0);
            black_box(native);
        }));
        let descriptor = Descriptor::new(schema.clone(), vec![2, 3]).unwrap();
        let values = vec![Value::String(text), Value::Bytes(bytes)];
        let exact = descriptor.preflight(&values).unwrap();
        let budget = TransportBudget::new(exact, exact);
        rows.push(measure("worker-typed-packet", size, iterations, || {
            let packet = descriptor
                .encode(
                    &values,
                    budget.reserve(TransportDirection::Input, exact).unwrap(),
                )
                .unwrap();
            black_box(packet.decode_typed(&descriptor).unwrap());
            drop(packet);
        }));
        assert_eq!(budget.snapshot().used_bytes, 0);
        rows.push(measure("string-backing-create", size, iterations, || {
            black_box(data.create_string(&mut heap, &"x".repeat(size)).unwrap());
        }));
    }
    let mut heap = DomainHeap::new(502);
    let mut data = ManagedData::new(502);
    data.configure_values(&heap, schema).unwrap();
    let scalar = ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, 42).unwrap());
    let list = data
        .create_composite(&mut heap, 1, None, &vec![scalar; 256])
        .unwrap();
    let index = ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, 0).unwrap());
    rows.push(measure("list256-immutable-set", 2048, iterations, || {
        black_box(
            data.operation(
                &mut heap,
                DataOperation::ListSet,
                4,
                &[1, 0, 0],
                &[ManagedSlot::Reference(list), index, scalar],
            )
            .unwrap(),
        );
    }));
    // A repeat always updates the original snapshot; growth is retained roots
    // before GC, not evidence of an uncollectable backing or native leak.
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"warmup":8,
        "scope":"Rust System allocations; synchronous typed transport packet, not thread scheduling or C malloc/RSS", "rows":rows})).unwrap());
}
