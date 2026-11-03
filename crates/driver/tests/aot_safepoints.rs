#[cfg(unix)]
#[test]
fn generated_helpers_preserve_poll_source_and_empty_root_semantics() {
    use std::path::Path;
    use std::process::Command;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = std::env::temp_dir().join(format!("rinzu-aot-safepoints-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    for flags in [
        vec!["-O2"],
        vec![
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ],
    ] {
        let binary = scratch.join("helpers");
        let build = Command::new("clang")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .args(flags)
            .arg("-I")
            .arg(root.join("../../include"))
            .arg(root.join("tests/fixtures/aot_safepoints.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let output = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .env("RINZU_GC_STRESS", "0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(scratch).unwrap();
}
