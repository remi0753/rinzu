#[test]
fn indexed_maps_preserve_collisions_order_immutability_gc_and_accounting() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("rinzu-map-index-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for sanitized in [false, true] {
        let binary = directory.join(if sanitized { "sanitized" } else { "plain" });
        let mut build = Command::new("clang");
        build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            build.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = build
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/managed_map_index.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = Command::new(binary)
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .env("RINZU_GC_STRESS", "0")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}
