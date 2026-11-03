use rinzu_runtime::data::{DataError, ManagedData, ManagedSlot};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::sync::Arc;
fn scalar(value: u64) -> ManagedSlot {
    ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value).unwrap())
}
fn configured(domain: u64) -> (DomainHeap, ManagedData) {
    let heap = DomainHeap::new(domain);
    let mut data = ManagedData::new(domain);
    let schema = ValueSchema::new(vec![
        ValueType::Scalar(ScalarKind::I64),
        ValueType::String,
        ValueType::State {
            identity: "Node".into(),
            fields: vec![0, 1, 3],
        },
        ValueType::Enum {
            identity: "Link".into(),
            variants: vec![vec![], vec![2]],
        },
    ])
    .unwrap();
    assert!(
        !schema.has_equality(2).unwrap()
            && !schema.has_transferable(2).unwrap()
            && !schema.is_key(2).unwrap()
    );
    assert!(!schema.has_transferable(3).unwrap());
    data.configure_values(&heap, Arc::new(schema)).unwrap();
    (heap, data)
}
#[test]
fn new_edges_self_and_mutual_cycles_survive_moving_gc_then_collect() {
    let (mut heap, mut data) = configured(900);
    let text = data.create_string(&mut heap, "old").unwrap();
    let none = data.create_composite(&mut heap, 3, Some(0), &[]).unwrap();
    let first = data
        .create_composite(
            &mut heap,
            2,
            None,
            &[
                scalar(1),
                ManagedSlot::Reference(text),
                ManagedSlot::Reference(none),
            ],
        )
        .unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    heap.collect(CollectionKind::Minor).unwrap();
    let next = data.create_string(&mut heap, "new-young").unwrap();
    data.set_state_field(&mut heap, first, 2, 1, ManagedSlot::Reference(next))
        .unwrap();
    let self_link = data
        .create_composite(&mut heap, 3, Some(1), &[ManagedSlot::Reference(first)])
        .unwrap();
    data.set_state_field(&mut heap, first, 2, 2, ManagedSlot::Reference(self_link))
        .unwrap();
    data.retain_roots(&mut heap, [first]).unwrap();
    heap.collect(CollectionKind::Minor).unwrap();
    let ManagedSlot::Reference(after) = data.project(&mut heap, first, 2, None, 1).unwrap() else {
        panic!("reference")
    };
    assert_eq!(data.string(&heap, after).unwrap(), "new-young");
    let none = data.create_composite(&mut heap, 3, Some(0), &[]).unwrap();
    let second = data
        .create_composite(
            &mut heap,
            2,
            None,
            &[
                scalar(2),
                ManagedSlot::Reference(after),
                ManagedSlot::Reference(none),
            ],
        )
        .unwrap();
    let to_second = data
        .create_composite(&mut heap, 3, Some(1), &[ManagedSlot::Reference(second)])
        .unwrap();
    let to_first = data
        .create_composite(&mut heap, 3, Some(1), &[ManagedSlot::Reference(first)])
        .unwrap();
    data.set_state_field(&mut heap, first, 2, 2, ManagedSlot::Reference(to_second))
        .unwrap();
    data.set_state_field(&mut heap, second, 2, 2, ManagedSlot::Reference(to_first))
        .unwrap();
    data.retain_roots(&mut heap, [first]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let ManagedSlot::Reference(link) = data.project(&mut heap, first, 2, None, 2).unwrap() else {
        panic!("link")
    };
    let ManagedSlot::Reference(second) = data.project(&mut heap, link, 3, Some(1), 0).unwrap()
    else {
        panic!("node")
    };
    data.set_state_field(&mut heap, second, 2, 0, scalar(99))
        .unwrap();
    assert_eq!(
        data.project(&mut heap, second, 2, None, 0).unwrap(),
        scalar(99)
    );
    data.release_all(&mut heap).unwrap();
    let report = heap.collect(CollectionKind::Full).unwrap();
    assert_eq!((heap.object_count(), data.root_count()), (0, 0));
    assert!(report.native_finalizers.is_empty());
}
#[test]
fn wrong_domain_owner_schema_slot_and_index_are_rejected_before_mutation() {
    let (mut heap, mut data) = configured(901);
    let (mut other, mut other_data) = configured(902);
    let text = data.create_string(&mut heap, "owner").unwrap();
    let none = data.create_composite(&mut heap, 3, Some(0), &[]).unwrap();
    let state = data
        .create_composite(
            &mut heap,
            2,
            None,
            &[
                scalar(1),
                ManagedSlot::Reference(text),
                ManagedSlot::Reference(none),
            ],
        )
        .unwrap();
    assert_eq!(
        data.set_state_field(&mut other, state, 2, 0, scalar(9)),
        Err(DataError::WrongDomain)
    );
    assert_eq!(
        other_data.set_state_field(&mut other, state, 2, 0, scalar(9)),
        Err(DataError::UnknownHandle)
    );
    assert!(
        data.set_state_field(&mut heap, state, 2, 9, scalar(9))
            .is_err()
    );
    assert!(
        data.set_state_field(&mut heap, state, 2, 1, scalar(9))
            .is_err()
    );
    assert_eq!(
        data.project(&mut heap, state, 2, None, 0).unwrap(),
        scalar(1)
    );
    let heap_address = std::ptr::from_mut(&mut heap) as usize;
    let data_address = std::ptr::from_mut(&mut data) as usize;
    let result = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                // SAFETY: the scope joins before either stack value can move,
                // and the parent does not access them until the join. No loan
                // is active. This deliberately bypasses Send to test rejection
                // of a foreign Owner before any managed payload is accessed.
                unsafe {
                    (&mut *(data_address as *mut ManagedData)).set_state_field(
                        &mut *(heap_address as *mut DomainHeap),
                        state,
                        2,
                        0,
                        scalar(9),
                    )
                }
            })
            .join()
            .unwrap()
    });
    assert!(matches!(result, Err(DataError::Heap(_))));
    assert_eq!(
        data.project(&mut heap, state, 2, None, 0).unwrap(),
        scalar(1)
    );
}

#[test]
fn generated_c_state_updates_enforce_domain_owner_and_collect_cycles() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("rinzu-state-native-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for sanitizer in [false, true] {
        let binary = directory.join(if sanitizer { "san" } else { "plain" });
        let mut command = std::process::Command::new("clang");
        command
            .args([
                "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/m2_state.c"))
            .arg("-o")
            .arg(&binary);
        if sanitizer {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let built = command.output().unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let run = std::process::Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
