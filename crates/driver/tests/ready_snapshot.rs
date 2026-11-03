#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn ready_snapshot_keeps_order_and_isolates_nested_turns_at_stack_heap_boundary() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::env::temp_dir().join(format!("rinzu-ready-snapshot-{}", std::process::id()));
    let output = std::process::Command::new("python3")
        .arg(root.join("tests/integration/test_ready_snapshot.py"))
        .arg("--workdir")
        .arg(&work)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(work);
    assert!(output.status.success(), "{output:?}");
}
