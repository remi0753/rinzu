#[cfg(unix)]
#[test]
fn final_byte_payloads_preserve_snapshots_gc_ownership_and_accounting() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("rinzu-managed-bytes-{}", std::process::id()));
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
            .arg(root.join("crates/runtime/tests/fixtures/managed_bytes.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for enabled in ["0", "1"] {
            let output = Command::new(&binary)
                .env("RINZU_MANAGED_INLINE_BYTES", enabled)
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert!(output.status.success(), "inline={enabled}: {output:?}");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
