#[test]
fn weak_lookup_hints_survive_relocation_and_expire_on_gc_and_rollback() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = std::env::temp_dir().join(format!("rinzu-lookup-cache-{}", std::process::id()));
    for sanitized in [false, true] {
        let mut build = Command::new("clang");
        build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            build.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = build
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/managed_lookup_cache.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for enabled in ["0", "1"] {
            let output = Command::new(&binary)
                .env("RINZU_MANAGED_LOOKUP_CACHE", enabled)
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{enabled}: {output:?}");
        }
    }
    std::fs::remove_file(binary).unwrap();
}
