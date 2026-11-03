use rinzu_runtime::data::{DataError, ManagedData, ManagedRoots, NativeOpaqueError};
use rinzu_runtime::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! { static ALLOCATION_BUDGET: Cell<isize> = const { Cell::new(-1) }; }
struct ControlledAllocator;
fn allowed() -> bool {
    ALLOCATION_BUDGET
        .try_with(|budget| match budget.get() {
            -1 => true,
            0 => false,
            count => {
                budget.set(count - 1);
                true
            }
        })
        .unwrap_or(true)
}
// SAFETY: all successful allocations/deallocations delegate unchanged to System;
// the thread-local test budget only injects the documented allocation failure.
unsafe impl GlobalAlloc for ControlledAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !allowed() {
            return std::ptr::null_mut();
        }
        // SAFETY: GlobalAlloc supplies the allocation layout to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: allocations retain the original System layout and pointer.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if !allowed() {
            return std::ptr::null_mut();
        }
        // SAFETY: realloc preserves the System allocator contract.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: ControlledAllocator = ControlledAllocator;

fn run(domain: u64) {
    let mut heap = DomainHeap::new(domain);
    let mut data = ManagedData::new(domain);
    let mut roots = ManagedRoots::default();
    // An unrooted predecessor makes the reserved wrapper actually move.
    heap.allocate(0, &[HeapValue::Integer(123)], FinalizerPolicy::None)
        .unwrap();
    let mut reservation = data.reserve_native_opaque(&mut heap, 7).unwrap();
    let unpublished = reservation.root().unwrap();
    assert_eq!(
        data.native_opaque_pointer(&heap, unpublished, 7),
        Err(DataError::WrongKind)
    );
    roots.enter_frame();
    roots.publish([unpublished]).unwrap();
    data.retain_roots(&mut heap, roots.live()).unwrap();
    assert!(heap.collect(CollectionKind::Full).unwrap().moved_objects > 0);
    // Deliberately non-dereferenceable bits must be preserved without probing.
    // Their resemblance to a managed identity must not add a GC reference.
    let address = unpublished.bits();
    ALLOCATION_BUDGET.with(|budget| budget.set(0));
    let received = data.receive_native_opaque(&mut heap, &mut reservation, address);
    let pointer = data.native_opaque_pointer(&heap, received.as_ref().copied().unwrap(), 7);
    ALLOCATION_BUDGET.with(|budget| budget.set(-1));
    let handle = received.unwrap();
    assert_eq!(pointer.unwrap(), address);
    assert_eq!(handle, unpublished);
    assert!(reservation.root().is_none());
    assert_eq!(
        data.native_opaque_pointer(&heap, handle, 8),
        Err(DataError::WrongKind)
    );
    let mut other_heap = DomainHeap::new(domain + 100);
    assert_eq!(
        data.native_opaque_pointer(&other_heap, handle, 7),
        Err(DataError::WrongDomain)
    );
    assert_eq!(
        data.reserve_native_opaque(&mut other_heap, 7).unwrap_err(),
        DataError::WrongDomain
    );

    let mut duplicate = data.reserve_native_opaque(&mut heap, 7).unwrap();
    let duplicate = data
        .receive_native_opaque(&mut heap, &mut duplicate, address)
        .unwrap();
    assert_ne!(duplicate, handle); // no deduplication by native address
    let mut null = data.reserve_native_opaque(&mut heap, 7).unwrap();
    let null_root = null.root().unwrap();
    ALLOCATION_BUDGET.with(|budget| budget.set(0));
    let null_result = data.receive_native_opaque(&mut heap, &mut null, 0);
    ALLOCATION_BUDGET.with(|budget| budget.set(-1));
    assert_eq!(null_result, Err(NativeOpaqueError::Null));
    assert!(null.root().is_none());
    assert_eq!(
        data.native_opaque_pointer(&heap, null_root, 7),
        Err(DataError::UnknownHandle)
    );
    let mut cancelled = data.reserve_native_opaque(&mut heap, 7).unwrap();
    data.discard_native_opaque(&mut heap, &mut cancelled)
        .unwrap();
    assert_eq!(
        data.discard_native_opaque(&mut heap, &mut cancelled),
        Err(DataError::UnknownHandle)
    );
    roots.publish([handle, duplicate]).unwrap();
    for _ in 0..32 {
        data.retain_roots(&mut heap, roots.live()).unwrap();
        let report = heap.collect(CollectionKind::Full).unwrap();
        assert!(report.native_finalizers.is_empty());
        assert_eq!(
            data.native_opaque_pointer(&heap, handle, 7).unwrap(),
            address
        );
        assert_eq!(
            data.native_opaque_pointer(&heap, duplicate, 7).unwrap(),
            address
        );
    }
    roots.leave_frame().unwrap();
    data.retain_roots(&mut heap, roots.live()).unwrap();
    let report = heap.collect(CollectionKind::Full).unwrap();
    assert!(report.native_finalizers.is_empty());
    assert_eq!(heap.object_count(), 0);
    assert_eq!(data.root_count(), 0);
}

#[test]
fn opaque_reservation_and_reception_are_domain_local_nonowning_and_allocation_free() {
    run(1);
    let workers: Vec<_> = (2..10)
        .map(|domain| std::thread::spawn(move || run(domain)))
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
}

#[test]
fn opaque_allocation_fault_child() {
    let Ok(budget) = std::env::var("RINZU_OPAQUE_ALLOCATION_BUDGET") else {
        return;
    };
    let budget = budget.parse().unwrap();
    let mut heap = DomainHeap::new(1);
    let mut data = ManagedData::new(1);
    ALLOCATION_BUDGET.with(|value| value.set(budget));
    let _reservation = data.reserve_native_opaque(&mut heap, 7).unwrap();
    // SAFETY: static marker is used only to detect an erroneous external entry
    // after a failed preallocation, without allocating in the child.
    unsafe {
        libc::write(1, b"NATIVE_ENTRY\n".as_ptr().cast(), 13);
    }
}

#[test]
fn opaque_fields_keep_nominal_wrappers_alive_through_parent_and_projection_roots() {
    use rinzu_runtime::data::ManagedSlot;
    use rinzu_runtime::value::{ValueSchema, ValueType};
    let mut heap = DomainHeap::new(10);
    let mut data = ManagedData::new(10);
    data.configure_values(
        &heap,
        std::sync::Arc::new(
            ValueSchema::new(vec![
                ValueType::Other,
                ValueType::Record {
                    identity: "Holder".into(),
                    fields: vec![0],
                },
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    let mut reservation = data.reserve_native_opaque(&mut heap, 0).unwrap();
    let opaque = data
        .receive_native_opaque(&mut heap, &mut reservation, 1)
        .unwrap();
    let parent = data
        .create_composite(&mut heap, 1, None, &[ManagedSlot::Reference(opaque)])
        .unwrap();
    assert!(
        data.snapshot(&heap, 1, ManagedSlot::Reference(parent))
            .is_err()
    );
    data.retain_roots(&mut heap, [parent]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let ManagedSlot::Reference(projected) = data.project(&mut heap, parent, 1, None, 0).unwrap()
    else {
        panic!("opaque field lost its managed reference");
    };
    data.retain_roots(&mut heap, [projected]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 1);
    assert_eq!(data.native_opaque_pointer(&heap, projected, 0).unwrap(), 1);
    data.release_all(&mut heap).unwrap();
    assert!(
        heap.collect(CollectionKind::Full)
            .unwrap()
            .native_finalizers
            .is_empty()
    );
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn opaque_preallocation_failures_are_fatal_before_native_entry() {
    for budget in 0..4 {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "opaque_allocation_fault_child", "--nocapture"])
            .env("RINZU_OPAQUE_ALLOCATION_BUDGET", budget.to_string())
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "allocation budget {budget} was not fatal"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("NATIVE_ENTRY"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_opaque_reservations_match_signed_and_sanitized_main_workers() {
    use std::process::Command;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory =
        std::env::temp_dir().join(format!("rinzu-native-opaque-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for sanitize in [false, true] {
        let binary = directory.join(if sanitize { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/native_opaque.c"));
        if sanitize {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !sanitize {
            for args in [
                vec!["--force", "--sign", "-", "--options", "runtime"],
                vec!["--verify", "--strict"],
            ] {
                let output = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        let output = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("PASS opaque Main/Worker"));
        for mode in [
            "oom",
            "wrong-type",
            "uninitialized",
            "wrong-domain",
            "foreign-thread",
        ] {
            let output = Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode == "oom" { 71 } else { 86 }),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty(), "{mode} entered native body");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
