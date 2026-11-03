//! Isolated baseline-runtime costs. Build once per revision, then compare the
//! frozen executables with tools/compare_runtime_costs.py (no Cargo in timing).
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use rinzu_runtime::data::{ManagedData, ManagedSlot};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{Value, ValueSchema, ValueType};

struct Allocator;
static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn record(size: usize, pointer: *mut u8) {
    if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(size as u64, Ordering::Relaxed);
    }
}

// SAFETY: every operation forwards the caller's layout and pointer to System;
// successful allocations only update process-local diagnostic counters.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplied a valid allocation layout.
        let pointer = unsafe { System.alloc(layout) };
        record(layout.size(), pointer);
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller supplied a valid allocation layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record(layout.size(), pointer);
        pointer
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller supplied the live allocation and replacement size.
        let result = unsafe { System.realloc(pointer, layout, size) };
        record(size, result);
        result
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller supplied the live allocation's pointer and layout.
        unsafe { System.dealloc(pointer, layout) };
    }
}

fn gc_case(objects: usize, width: usize, rounds: usize) -> u64 {
    let mut heap = DomainHeap::new(730);
    let mut previous = None;
    for value in 0..objects {
        let mut fields = vec![HeapValue::Integer(value as i64); width];
        if let Some(reference) = previous {
            fields[0] = HeapValue::Reference(reference);
        }
        previous = Some(heap.allocate(7, &fields, FinalizerPolicy::None).unwrap());
    }
    let root = heap.create_root(previous).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let measured = measure(|| {
        for round in 0..rounds {
            let report = heap
                .collect(if round % 2 == 0 {
                    CollectionKind::Full
                } else {
                    CollectionKind::Minor
                })
                .unwrap();
            assert_eq!(report.moved_objects, objects);
            assert_eq!(report.reclaimed_objects, 0);
        }
    });
    // Verify all relocated edges and payloads outside the measured interval.
    let mut reference = heap.root_target(root).unwrap();
    let mut checksum = 0;
    for value in (0..objects).rev() {
        let current = reference.unwrap();
        let fields = heap.fields(current).unwrap();
        assert_eq!(fields.len(), width);
        assert!(heap.header(current).unwrap().is_old);
        for field in &fields[1..] {
            assert_eq!(*field, HeapValue::Integer(value as i64));
            checksum += value as u64;
        }
        reference = match fields[0] {
            HeapValue::Reference(next) => Some(next),
            HeapValue::Integer(0) if value == 0 => None,
            _ => panic!("invalid relocated edge"),
        };
    }
    assert!(reference.is_none());
    emit(&format!("gc-{objects}x{width}"), rounds, measured, checksum);
    checksum
}

fn repetitions() -> usize {
    static REPETITIONS: OnceLock<usize> = OnceLock::new();
    *REPETITIONS.get_or_init(|| {
        let value = std::env::var("RINZU_PROBE_REPETITIONS")
            .map(|text| text.parse::<usize>().expect("probe repetition count"))
            .unwrap_or(1);
        assert!((1..=100).contains(&value));
        value
    })
}

fn cpu_nanoseconds() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        // SAFETY: now is a writable timespec; the process CPU clock has no handles.
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut now) },
        0
    );
    (now.tv_sec as u64) * 1_000_000_000 + now.tv_nsec as u64
}

fn measure(mut run: impl FnMut()) -> (u128, u64, u64, u64) {
    // Timing and allocation diagnostics are separate passes. The timer pass
    // does no counter increments. Setup, warmup and validation are excluded.
    let repetitions = repetitions();
    let cpu_start = cpu_nanoseconds();
    let start = Instant::now();
    for _ in 0..repetitions {
        run();
    }
    let nanos = start.elapsed().as_nanos();
    let cpu_nanos = cpu_nanoseconds() - cpu_start;
    ALLOCATIONS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    for _ in 0..repetitions {
        run();
    }
    COUNTING.store(false, Ordering::Relaxed);
    (
        nanos,
        ALLOCATIONS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
        cpu_nanos,
    )
}

fn emit(case: &str, operations: usize, measured: (u128, u64, u64, u64), checksum: u64) {
    println!(
        "{}",
        serde_json::json!({
            "case": case, "operations": operations * repetitions(), "nanoseconds": measured.0,
            "cpu_nanoseconds": measured.3,
            "allocations": measured.1, "allocated_bytes": measured.2, "checksum": checksum,
        })
    );
}

fn packed_case(length: usize, rounds: usize) {
    let mut heap = DomainHeap::new(731);
    let mut data = ManagedData::new(731);
    let text = "Aé🙂z".repeat(length / 8);
    assert_eq!(text.len(), length);
    let left = data.create_string(&mut heap, &text).unwrap();
    let right = data.create_string(&mut heap, &text).unwrap();
    assert_ne!(left, right);
    let bytes = data
        .create_bytes(&mut heap, length as u64, text.as_bytes())
        .unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    for op in ["byte-length", "string-equal", "string-hash"] {
        let expected = match op {
            "byte-length" => length as u64,
            "string-equal" => 1,
            _ => rinzu_runtime::key::string_hash(&text),
        };
        let mut checksum = 0_u64;
        let measured = measure(|| {
            checksum = 0;
            for _ in 0..rounds {
                let value = match op {
                    "byte-length" => data.byte_length(&heap, black_box(bytes), u64::MAX).unwrap(),
                    "string-equal" => data
                        .strings_equal(&heap, black_box(left), black_box(right))
                        .unwrap() as u64,
                    _ => data.string_hash(&heap, black_box(left)).unwrap(),
                };
                checksum = checksum.wrapping_add(black_box(value));
            }
        });
        assert_eq!(checksum, expected.wrapping_mul(rounds as u64));
        emit(&format!("{op}-{length}"), rounds, measured, checksum);
    }
}

fn materialize_case(resident: usize, rounds: usize) {
    let mut heap = DomainHeap::new(732);
    let mut data = ManagedData::new(732);
    data.configure_values(
        &heap,
        std::sync::Arc::new(ValueSchema::new(vec![ValueType::String]).unwrap()),
    )
    .unwrap();
    let roots: Vec<_> = (0..resident)
        .map(|_| data.create_string(&mut heap, "resident🙂").unwrap())
        .collect();
    let values = [Value::String("input\0日本語".into())];
    heap.collect(CollectionKind::Full).unwrap();
    let mut last = None;
    let measured = measure(|| {
        for _ in 0..rounds {
            let received = data
                .materialize_values(&mut heap, &[0], black_box(&values))
                .unwrap();
            let [ManagedSlot::Reference(handle)] = received.as_slice() else {
                panic!("expected String")
            };
            last = Some(*handle);
            data.release(&mut heap, *handle).unwrap();
        }
    });
    assert_eq!(data.root_count(), resident);
    assert!(data.string(&heap, last.unwrap()).is_err());
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), resident);
    let mut checksum = 0;
    for root in roots {
        assert_eq!(data.string(&heap, root).unwrap(), "resident🙂");
        checksum += data.string_length(&heap, root).unwrap() as u64;
    }
    let received = data.materialize_values(&mut heap, &[0], &values).unwrap();
    assert!(
        matches!(data.snapshot(&heap, 0, received[0]).unwrap(), Value::String(text) if text == "input\0日本語")
    );
    emit(
        &format!("materialize-{resident}-resident"),
        rounds,
        measured,
        checksum,
    );
}

fn composite_case(width: usize, rounds: usize) {
    let mut heap = DomainHeap::new(733);
    let mut data = ManagedData::new(733);
    let schema = ValueSchema::new(vec![
        ValueType::Scalar(ScalarKind::U64),
        ValueType::Record {
            identity: "ProbeRecord".into(),
            fields: vec![0; width],
        },
        ValueType::Enum {
            identity: "ProbeEnum".into(),
            variants: vec![vec![0; width], vec![]],
        },
        ValueType::State {
            identity: "ProbeState".into(),
            fields: vec![0; width],
        },
        ValueType::List(0),
    ])
    .unwrap();
    data.configure_values(&heap, std::sync::Arc::new(schema))
        .unwrap();
    let fields: Vec<_> = (0..width)
        .map(|n| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::U64, n as u64).unwrap()))
        .collect();
    let record = data.create_composite(&mut heap, 1, None, &fields).unwrap();
    let variant = data
        .create_composite(&mut heap, 2, Some(0), &fields)
        .unwrap();
    let state = data.create_composite(&mut heap, 3, None, &fields).unwrap();
    let list = data.create_composite(&mut heap, 4, None, &fields).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    for op in ["project", "variant", "state-write", "list-length"] {
        let mut checksum = 0_u64;
        let measured = measure(|| {
            checksum = 0;
            for _ in 0..rounds {
                let value = match op {
                    "project" => {
                        let ManagedSlot::Scalar(value) = data
                            .project(&mut heap, black_box(record), 1, None, width - 1)
                            .unwrap()
                        else {
                            panic!("scalar")
                        };
                        value.bits()
                    }
                    "variant" => data.enum_variant(&heap, black_box(variant), 2).unwrap() as u64,
                    "state-write" => {
                        data.set_state_field(
                            &mut heap,
                            black_box(state),
                            3,
                            width - 1,
                            fields[width - 1],
                        )
                        .unwrap();
                        (width - 1) as u64
                    }
                    _ => {
                        let ManagedSlot::Scalar(value) = data
                            .operation(
                                &mut heap,
                                rinzu_abi::data_operation::DataOperation::ListLength,
                                0,
                                &[4],
                                &[ManagedSlot::Reference(black_box(list))],
                            )
                            .unwrap()
                        else {
                            panic!("length")
                        };
                        value.bits()
                    }
                };
                checksum = checksum.wrapping_add(black_box(value));
            }
        });
        let expected = match op {
            "variant" => 0,
            "list-length" => width,
            _ => width - 1,
        } as u64;
        assert_eq!(checksum, expected * rounds as u64);
        emit(&format!("{op}-{width}"), rounds, measured, checksum);
    }
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        data.project(&mut heap, state, 3, None, width - 1).unwrap(),
        fields[width - 1]
    );
}

fn collection_case(width: usize, rounds: usize, map: bool, missing: bool) {
    use rinzu_abi::data_operation::DataOperation;
    let mut heap = DomainHeap::new(734);
    let mut data = ManagedData::new(734);
    data.configure_values(
        &heap,
        std::sync::Arc::new(
            ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::I64),
                ValueType::List(0),
                ValueType::Map { key: 0, value: 0 },
                ValueType::Enum {
                    identity: "Option<Int>".into(),
                    variants: vec![vec![], vec![0]],
                },
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    let scalar =
        |n: i64| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n as u64).unwrap());
    let mut fields = Vec::new();
    for i in 0..width {
        fields.push(scalar(i as i64));
        if map {
            fields.push(scalar((i * 7) as i64));
        }
    }
    let ty = if map { 2 } else { 1 };
    let container = data.create_composite(&mut heap, ty, None, &fields).unwrap();
    let query = scalar(if missing { -1 } else { (width - 1) as i64 });
    let op = if map {
        DataOperation::MapGet
    } else {
        DataOperation::ListGet
    };
    let mut checksum = 0_u64;
    let measured = measure(|| {
        checksum = 0;
        for _ in 0..rounds {
            let result = data
                .operation(
                    &mut heap,
                    op,
                    3,
                    &[ty, 0],
                    &[ManagedSlot::Reference(black_box(container)), query],
                )
                .unwrap();
            let ManagedSlot::Reference(handle) = result else {
                panic!("Option")
            };
            checksum += data.enum_variant(&heap, handle, 3).unwrap() as u64;
            data.release(&mut heap, handle).unwrap();
        }
    });
    assert_eq!(checksum, if missing { 0 } else { rounds as u64 });
    assert_eq!(data.root_count(), 1);
    let result = data
        .operation(
            &mut heap,
            op,
            3,
            &[ty, 0],
            &[ManagedSlot::Reference(container), query],
        )
        .unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let Value::Enum {
        variant, fields, ..
    } = data.snapshot(&heap, 3, result).unwrap()
    else {
        panic!("Option")
    };
    assert_eq!(variant, u32::from(!missing));
    if !missing {
        assert!(
            matches!(&fields[..], [Value::Scalar(value)] if value.bits() == ((width - 1) * if map { 7 } else { 1 }) as u64)
        );
    }
    emit(
        &format!(
            "{}-{width}-{}",
            if map { "map-get" } else { "list-get" },
            if missing { "miss" } else { "hit" }
        ),
        rounds,
        measured,
        checksum,
    );
}

fn construction_case(length: usize, rounds: usize, kind: &str) {
    let mut heap = DomainHeap::new(735);
    let mut data = ManagedData::new(735);
    let text = "Aé🙂z".repeat(length / 8);
    assert_eq!(text.len(), length);
    let mut native_text = text.as_bytes().to_vec();
    native_text.push(0);
    let bytes: Vec<_> = (0..length).map(|n| (n % 256) as u8).collect();
    let create = |data: &mut ManagedData, heap: &mut DomainHeap| match kind {
        "string" => data.create_string(heap, black_box(&text)).unwrap(),
        "bytes" => data
            .create_bytes(heap, length as u64, black_box(&bytes))
            .unwrap(),
        "native-string" => {
            // SAFETY: this immutable, terminated buffer stays live through copy.
            unsafe { data.receive_native_string(heap, black_box(native_text.as_ptr())) }.unwrap()
        }
        _ => {
            // SAFETY: the complete fixed-length buffer stays live through copy.
            unsafe { data.receive_native_bytes(heap, length as u64, black_box(bytes.as_ptr())) }
                .unwrap()
        }
    };
    let measured = measure(|| {
        for _ in 0..rounds {
            let handle = create(&mut data, &mut heap);
            black_box(handle);
            data.release(&mut heap, handle).unwrap();
            if heap.collection_due() {
                heap.collect(CollectionKind::Full).unwrap();
            }
        }
    });
    assert_eq!(data.root_count(), 0);
    let handle = create(&mut data, &mut heap);
    heap.collect(CollectionKind::Full).unwrap();
    if kind.ends_with("string") {
        assert_eq!(data.string(&heap, handle).unwrap(), text);
    } else {
        assert_eq!(data.bytes(&heap, handle, length as u64).unwrap(), bytes);
    }
    emit(
        &format!("construct-{kind}-{length}"),
        rounds,
        measured,
        length as u64,
    );
}

fn byte_read_case(length: usize, rounds: usize, kind: &str) {
    use rinzu_abi::data_operation::DataOperation;
    let mut heap = DomainHeap::new(737);
    let mut data = ManagedData::new(737);
    data.configure_values(
        &heap,
        std::sync::Arc::new(
            ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::I64),
                ValueType::Scalar(ScalarKind::U8),
                ValueType::DynamicBytes,
                ValueType::Enum {
                    identity: "Option<Int>".into(),
                    variants: vec![vec![], vec![0]],
                },
                ValueType::Enum {
                    identity: "Option<U8>".into(),
                    variants: vec![vec![], vec![1]],
                },
                ValueType::List(1),
                ValueType::Enum {
                    identity: "Option<Bytes>".into(),
                    variants: vec![vec![], vec![2]],
                },
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    let mut bytes = vec![0x41; length];
    if let Some(last) = bytes.last_mut() {
        *last = 0xff;
    }
    let input = data.create_bytes(&mut heap, length as u64, &bytes).unwrap();
    let scalar = |n| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n).unwrap());
    let set = data
        .create_bytes(
            &mut heap,
            1,
            if kind == "not-any-last" {
                &[0x41]
            } else if kind == "find-miss" {
                &[0xfe]
            } else {
                &[0xff]
            },
        )
        .unwrap();
    let (op, result_type, types, slots) = match kind {
        "get-last" | "get-miss" => (
            DataOperation::BytesGet,
            4,
            vec![2, 0],
            vec![
                ManagedSlot::Reference(input),
                scalar(if kind == "get-miss" {
                    length
                } else {
                    length.saturating_sub(1)
                } as u64),
            ],
        ),
        "slice" => (
            DataOperation::BytesSlice,
            6,
            vec![2, 0, 0],
            vec![
                ManagedSlot::Reference(input),
                scalar(0),
                scalar(length.min(8) as u64),
            ],
        ),
        "array" => {
            let children: Vec<_> = bytes
                .iter()
                .map(|byte| {
                    ManagedSlot::Scalar(
                        ScalarValue::from_bits(ScalarKind::U8, u64::from(*byte)).unwrap(),
                    )
                })
                .collect();
            let list = data
                .create_composite(&mut heap, 5, None, &children)
                .unwrap();
            (
                DataOperation::ByteArray,
                2,
                vec![5],
                vec![ManagedSlot::Reference(list)],
            )
        }
        _ => (
            if kind == "not-any-last" {
                DataOperation::BytesFindNotAny
            } else {
                DataOperation::BytesFindAny
            },
            3,
            vec![2, 2, 0],
            vec![
                ManagedSlot::Reference(input),
                ManagedSlot::Reference(set),
                scalar(0),
            ],
        ),
    };
    let expected_some =
        kind == "slice" || kind == "array" || (length != 0 && !kind.ends_with("miss"));
    let mut checksum = 0;
    let measured = measure(|| {
        checksum = 0;
        for _ in 0..rounds {
            let ManagedSlot::Reference(handle) = data
                .operation(&mut heap, op, result_type, &types, black_box(&slots))
                .unwrap()
            else {
                panic!("result")
            };
            checksum += if kind == "array" {
                data.dynamic_byte_length(&heap, handle).unwrap()
            } else {
                u64::from(data.enum_variant(&heap, handle, result_type).unwrap())
            };
            data.release(&mut heap, handle).unwrap();
            if heap.collection_due() {
                heap.collect(CollectionKind::Full).unwrap();
            }
        }
    });
    assert_eq!(
        checksum,
        rounds as u64
            * if kind == "array" {
                length as u64
            } else {
                u64::from(expected_some)
            }
    );
    let result = data
        .operation(&mut heap, op, result_type, &types, &slots)
        .unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    match data.snapshot(&heap, result_type, result).unwrap() {
        Value::Bytes(value) => assert_eq!(value, bytes),
        Value::Enum {
            variant, fields, ..
        } => {
            assert_eq!(variant, u32::from(expected_some));
            if expected_some {
                if kind == "slice" {
                    assert!(
                        matches!(&fields[0], Value::Bytes(value) if *value == bytes[..length.min(8)])
                    );
                } else {
                    assert!(
                        matches!(fields[0], Value::Scalar(value) if value.bits() == if kind == "get-last" { 0xff } else { (length - 1) as u64 })
                    );
                }
            }
        }
        _ => panic!("unexpected value"),
    }
    emit(
        &format!("bytes-{kind}-{length}"),
        rounds,
        measured,
        checksum,
    );
}

fn collection_update_case(width: usize, rounds: usize, kind: &str) {
    use rinzu_abi::data_operation::DataOperation;
    let mut heap = DomainHeap::new(738);
    let mut data = ManagedData::new(738);
    let schema = std::sync::Arc::new(
        ValueSchema::new(vec![
            ValueType::String,
            ValueType::Scalar(ScalarKind::I64),
            ValueType::List(0),
            ValueType::Map { key: 1, value: 0 },
            ValueType::Enum {
                identity: "Option<List<String>>".into(),
                variants: vec![vec![], vec![2]],
            },
            ValueType::List(1),
        ])
        .unwrap(),
    );
    data.configure_values(&heap, schema.clone()).unwrap();
    let scalar =
        |n: i64| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n as u64).unwrap());
    let is_map = kind.starts_with("map-");
    let ty = if is_map { 3 } else { 2 };
    let mut fields = Vec::with_capacity(width * if is_map { 2 } else { 1 });
    for i in 0..width {
        if is_map {
            fields.push(scalar(i as i64));
        }
        fields.push(ManagedSlot::Reference(
            data.create_string(&mut heap, &format!("value-{i}"))
                .unwrap(),
        ));
    }
    let parent = data.create_composite(&mut heap, ty, None, &fields).unwrap();
    let tail = data.create_string(&mut heap, "tail").unwrap();
    data.retain_roots(&mut heap, [parent, tail]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let middle = width / 2;
    let (op, result_ty, types, slots) = match kind {
        "list-append" => (
            DataOperation::ListAppend,
            2,
            vec![2, 0],
            vec![ManagedSlot::Reference(parent), ManagedSlot::Reference(tail)],
        ),
        "list-set" => (
            DataOperation::ListSet,
            4,
            vec![2, 1, 0],
            vec![
                ManagedSlot::Reference(parent),
                scalar(middle as i64),
                ManagedSlot::Reference(tail),
            ],
        ),
        "list-remove" | "list-remove-miss" => (
            DataOperation::ListRemove,
            4,
            vec![2, 1],
            vec![
                ManagedSlot::Reference(parent),
                scalar(if kind.ends_with("miss") {
                    -1
                } else {
                    middle as i64
                }),
            ],
        ),
        "map-insert" => (
            DataOperation::MapInsert,
            3,
            vec![3, 1, 0],
            vec![
                ManagedSlot::Reference(parent),
                scalar(middle as i64),
                ManagedSlot::Reference(tail),
            ],
        ),
        "map-remove" => (
            DataOperation::MapRemove,
            3,
            vec![3, 1],
            vec![ManagedSlot::Reference(parent), scalar(middle as i64)],
        ),
        "map-keys" => (
            DataOperation::MapKeys,
            5,
            vec![3],
            vec![ManagedSlot::Reference(parent)],
        ),
        _ => panic!("unknown collection update"),
    };
    let mut peak_roots = 0;
    let mut checksum = 0;
    let measured = measure(|| {
        peak_roots = 0;
        checksum = 0;
        for _ in 0..rounds {
            let ManagedSlot::Reference(handle) = data
                .operation(&mut heap, op, result_ty, &types, black_box(&slots))
                .unwrap()
            else {
                panic!()
            };
            black_box(handle);
            peak_roots = peak_roots.max(data.root_count());
            checksum += 1;
            data.release(&mut heap, handle).unwrap();
            if heap.collection_due() {
                // Match baseline safepoint publication, including its cost.
                data.retain_roots(&mut heap, [parent, tail]).unwrap();
                heap.collect(CollectionKind::Full).unwrap();
            }
        }
    });
    let output = data
        .operation(&mut heap, op, result_ty, &types, &slots)
        .unwrap();
    let ManagedSlot::Reference(handle) = output else {
        panic!()
    };
    data.retain_roots(&mut heap, [handle]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let actual = data.snapshot(&heap, result_ty, output).unwrap();
    let mut expected: Vec<_> = (0..width)
        .map(|i| Value::String(format!("value-{i}")))
        .collect();
    let expected = match kind {
        "list-append" => {
            expected.push(Value::String("tail".into()));
            Value::List(expected)
        }
        "list-set" | "list-remove" => {
            if kind == "list-set" {
                expected[middle] = Value::String("tail".into());
            } else {
                expected.remove(middle);
            }
            Value::Enum {
                identity: "Option<List<String>>".into(),
                variant: 1,
                fields: vec![Value::List(expected)],
            }
        }
        "list-remove-miss" => Value::Enum {
            identity: "Option<List<String>>".into(),
            variant: 0,
            fields: vec![],
        },
        "map-keys" => Value::List(
            (0..width)
                .map(|i| Value::Scalar(ScalarValue::from_bits(ScalarKind::I64, i as u64).unwrap()))
                .collect(),
        ),
        _ => {
            if kind == "map-insert" {
                expected[middle] = Value::String("tail".into());
            }
            let expected: Vec<_> = expected
                .into_iter()
                .enumerate()
                .filter(|(i, _)| kind != "map-remove" || *i != middle)
                .map(|(i, value)| {
                    (
                        Value::Scalar(ScalarValue::from_bits(ScalarKind::I64, i as u64).unwrap()),
                        value,
                    )
                })
                .collect();
            Value::Map(expected)
        }
    };
    assert!(schema.equal(result_ty, &actual, &expected).unwrap());
    data.release(&mut heap, handle).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((data.root_count(), heap.object_count()), (0, 0));
    println!(
        "{}",
        serde_json::json!({"case":format!("{kind}-{width}"),"operations":rounds * repetitions(),"nanoseconds":measured.0,"cpu_nanoseconds":measured.3,"allocations":measured.1,"allocated_bytes":measured.2,"checksum":checksum,"peak_roots":peak_roots})
    );
}

fn map_construction_case(width: usize, rounds: usize, duplicates: bool) {
    let mut heap = DomainHeap::new(739);
    let mut data = ManagedData::new(739);
    data.configure_values(
        &heap,
        std::sync::Arc::new(
            ValueSchema::new(vec![
                ValueType::Scalar(ScalarKind::I64),
                ValueType::Map { key: 0, value: 0 },
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    let count = if duplicates { width * 2 } else { width };
    let scalar = |n| ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, n).unwrap());
    let mut fields = Vec::with_capacity(count * 2);
    for i in 0..count {
        fields.extend([scalar((i % width) as u64), scalar(i as u64)]);
    }
    let measured = measure(|| {
        for _ in 0..rounds {
            let handle = data
                .create_composite(&mut heap, 1, None, black_box(&fields))
                .unwrap();
            black_box(handle);
            data.release(&mut heap, handle).unwrap();
            if heap.collection_due() {
                heap.collect(CollectionKind::Full).unwrap();
            }
        }
    });
    let handle = data.create_composite(&mut heap, 1, None, &fields).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let Value::Map(pairs) = data
        .snapshot(&heap, 1, ManagedSlot::Reference(handle))
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(pairs.len(), width);
    for (i, (key, value)) in pairs.into_iter().enumerate() {
        assert!(
            matches!(key, Value::Scalar(value) if value.kind() == ScalarKind::I64 && value.bits() == i as u64)
        );
        assert!(
            matches!(value, Value::Scalar(value) if value.kind() == ScalarKind::I64 && value.bits() == (i + if duplicates {width} else {0}) as u64)
        );
    }
    emit(
        &format!(
            "map-construct-{width}-{}",
            if duplicates { "duplicates" } else { "unique" }
        ),
        rounds,
        measured,
        width as u64,
    );
}

fn primitive_cases() {
    let mut heap = DomainHeap::new(736);
    let mut data = ManagedData::new(736);
    let schema = std::sync::Arc::new(
        ValueSchema::new(vec![
            ValueType::Scalar(ScalarKind::I64),
            ValueType::String,
            ValueType::Record {
                identity: "Probe".into(),
                fields: vec![0, 1],
            },
        ])
        .unwrap(),
    );
    data.configure_values(&heap, schema.clone()).unwrap();
    let number = ScalarValue::from_bits(ScalarKind::I64, 7).unwrap();
    let string = data.create_string(&mut heap, "Aé🙂z").unwrap();
    let record = data
        .create_composite(
            &mut heap,
            2,
            None,
            &[ManagedSlot::Scalar(number), ManagedSlot::Reference(string)],
        )
        .unwrap();
    let cases = [
        (
            "integer",
            0,
            Value::Scalar(number),
            ManagedSlot::Scalar(number),
        ),
        (
            "string",
            1,
            Value::String("Aé🙂z".into()),
            ManagedSlot::Reference(string),
        ),
        (
            "record",
            2,
            Value::Record {
                identity: "Probe".into(),
                fields: vec![Value::Scalar(number), Value::String("Aé🙂z".into())],
            },
            ManagedSlot::Reference(record),
        ),
    ];
    heap.collect(CollectionKind::Full).unwrap();
    let rounds = 100000;
    for (name, ty, value, slot) in cases {
        let right = value.clone();
        for op in ["validate", "hash", "equal", "snapshot"] {
            let expected = match op {
                "hash" => schema.key_hash(ty, &value).unwrap(),
                "snapshot" => match &value {
                    Value::Scalar(v) => v.bits(),
                    Value::String(s) => s.len() as u64,
                    Value::Record { fields, .. } => fields.len() as u64,
                    _ => unreachable!(),
                },
                _ => 1,
            };
            let mut checksum = 0_u64;
            let measured = measure(|| {
                checksum = 0;
                for _ in 0..rounds {
                    let result = match op {
                        "validate" => {
                            schema.validate(ty, black_box(&value)).unwrap();
                            1
                        }
                        "hash" => schema.key_hash(ty, black_box(&value)).unwrap(),
                        "equal" => schema
                            .equal(ty, black_box(&value), black_box(&right))
                            .unwrap() as u64,
                        _ => match data.snapshot(&heap, ty, black_box(slot)).unwrap() {
                            Value::Scalar(v) => v.bits(),
                            Value::String(s) => s.len() as u64,
                            Value::Record { fields, .. } => fields.len() as u64,
                            _ => unreachable!(),
                        },
                    };
                    checksum = checksum.wrapping_add(black_box(result));
                }
            });
            assert_eq!(checksum, expected.wrapping_mul(rounds as u64));
            assert!(
                schema
                    .equal(ty, &data.snapshot(&heap, ty, slot).unwrap(), &value)
                    .unwrap()
            );
            emit(&format!("{op}-{name}"), rounds, measured, checksum);
        }
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("gc") => {
            black_box(gc_case(4096, 4, 30));
            black_box(gc_case(512, 1024, 12));
        }
        Some("packed") => {
            for (length, rounds) in [(0, 20000), (8, 20000), (65536, 2000)] {
                packed_case(length, rounds);
            }
        }
        Some("materialize") => {
            for (resident, rounds) in [(0, 10000), (1024, 2000), (16384, 1000)] {
                materialize_case(resident, rounds);
            }
        }
        Some("composite") => {
            for (width, rounds) in [(4, 20000), (4096, 2000)] {
                composite_case(width, rounds);
            }
        }
        Some("collection") => {
            for missing in [false, true] {
                collection_case(16, 20000, false, missing);
                collection_case(4096, 2000, false, missing);
                collection_case(16, 1000, true, missing);
                collection_case(1024, 100, true, missing);
            }
        }
        Some("construction") => {
            for kind in ["string", "bytes", "native-string", "native-bytes"] {
                for (length, rounds) in [(0, 10000), (8, 10000), (65536, 500)] {
                    construction_case(length, rounds, kind);
                }
            }
        }
        Some("primitive") => primitive_cases(),
        Some("bytes-reads") => {
            for (length, rounds) in [(0, 10000), (8, 10000), (65536, 1000)] {
                for kind in [
                    "get-last",
                    "get-miss",
                    "find-last",
                    "find-miss",
                    "not-any-last",
                    "slice",
                ] {
                    byte_read_case(length, rounds, kind);
                }
                byte_read_case(length, if length > 8 { 100 } else { rounds }, "array");
            }
        }
        Some("collection-updates") => {
            for (width, rounds) in [(16, 1000), (256, 200)] {
                for kind in [
                    "list-append",
                    "list-set",
                    "list-remove",
                    "list-remove-miss",
                    "map-insert",
                    "map-remove",
                    "map-keys",
                ] {
                    collection_update_case(width, rounds, kind);
                }
            }
            for duplicates in [false, true] {
                map_construction_case(256, 200, duplicates);
            }
        }
        _ => panic!(
            "usage: runtime_cost_probe gc|packed|materialize|composite|collection|construction|primitive|bytes-reads|collection-updates"
        ),
    }
}
