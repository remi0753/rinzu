#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn small_enum_cache_preserves_weak_lifetimes_quota_ownership_and_physical_cleanup() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("rinzu-small-enums-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for (mode, flags) in [
        ("plain", &[][..]),
        (
            "san",
            &["-fsanitize=address,undefined", "-fno-sanitize-recover=all"][..],
        ),
        ("tsan", &["-fsanitize=thread"][..]),
    ] {
        let binary = directory.join(mode);
        let output = Command::new("clang")
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .args(flags)
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/managed_small_enums.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        for (argument, lookup, expected) in
            [("normal", "1", 0), ("owner", "1", 70), ("normal", "0", 0)]
        {
            let output = Command::new(&binary)
                .arg(argument)
                .env("RINZU_MANAGED_LOOKUP_CACHE", lookup)
                .env_remove("RINZU_GC_STRESS")
                .env_remove("RINZU_RESOURCE_REPORT")
                .env_remove("RINZU_RESOURCE_ACCOUNTING")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{mode}/{argument}/{lookup}: {output:?}"
            );
            if expected == 0 {
                assert!(String::from_utf8_lossy(&output.stdout).contains("PASS small enum mode=2"));
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
