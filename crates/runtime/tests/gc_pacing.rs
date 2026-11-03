use rinzu_runtime::gc::MIN_ALLOCATION_BUDGET;
use rinzu_runtime::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};

#[test]
fn allocation_pressure_waits_for_roots_and_a_collection_resets_the_budget() {
    let mut heap = DomainHeap::new(101);
    let parent = heap
        .allocate(1, &[HeapValue::Integer(42)], FinalizerPolicy::None)
        .unwrap();
    let root = heap.create_root(Some(parent)).unwrap();
    assert!(!heap.collection_due());
    while heap.allocated_bytes() < MIN_ALLOCATION_BUDGET {
        heap.allocate(1, &[HeapValue::Integer(0)], FinalizerPolicy::None)
            .unwrap();
    }
    assert!(heap.collection_due());
    // Crossing the threshold did not itself move the unregistered local.
    assert_eq!(heap.fields(parent).unwrap(), vec![HeapValue::Integer(42)]);
    let report = heap.collect(CollectionKind::Full).unwrap();
    assert!(report.reclaimed_objects > 1000);
    assert_eq!(report.moved_objects, 1);
    assert!(!heap.collection_due());
    let parent = heap.root_target(root).unwrap().unwrap();
    assert_eq!(heap.fields(parent).unwrap(), vec![HeapValue::Integer(42)]);
    heap.release_root(root).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
    heap.enable_gc_stress();
    assert!(heap.collection_due());
}

#[cfg(unix)]
#[test]
fn c_pacing_preserves_parked_roots_borrows_and_ownership() {
    run_c_fixture("gc_pacing");
}

#[cfg(unix)]
#[test]
fn c_root_publication_reuses_storage_without_retaining_inactive_roots() {
    run_c_fixture("root_publication");
}

#[cfg(unix)]
#[test]
fn c_managed_text_caches_validation_and_preserves_moving_payloads() {
    run_c_fixture("managed_text");
}

#[cfg(unix)]
#[test]
fn c_literal_cache_is_weak_bounded_and_local_to_its_owner() {
    run_c_fixture("managed_literals");
}

#[cfg(unix)]
#[test]
fn c_byte_literal_cache_preserves_kinds_snapshots_gc_rollback_and_ownership() {
    run_c_fixture("managed_byte_literals");
}

#[cfg(unix)]
#[test]
fn c_survivor_storage_retains_normal_payloads_and_moves_under_stress() {
    run_c_fixture_with_arguments("gc_relocation", &["retain"]);
    run_c_fixture_with_arguments("gc_relocation", &["stress"]);
}

#[cfg(unix)]
fn run_c_fixture(name: &str) {
    run_c_fixture_with_arguments(name, &[]);
}

#[cfg(unix)]
fn run_c_fixture_with_arguments(name: &str, arguments: &[&str]) {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = std::env::temp_dir().join(format!("rinzu-{name}-{}", std::process::id()));
    let build = Command::new("clang")
        .args([
            "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
        ])
        .arg(root.join("include"))
        .arg(root.join(format!("crates/runtime/tests/fixtures/{name}.c")))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&binary).args(arguments).output().unwrap();
    let _ = std::fs::remove_file(&binary);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
}
