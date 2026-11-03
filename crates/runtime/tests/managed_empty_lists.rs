#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn explicitly_charged_empty_lists_preserve_weak_ownership_and_fresh_callers() {
    use std::{path::Path, process::Command};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("rinzu-empty-lists-{}", std::process::id()));
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
            .arg(root.join("crates/runtime/tests/fixtures/managed_empty_lists.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        for (owner, lookup, expected) in [(false, "1", 0), (false, "0", 0), (true, "1", 70)] {
            let mut command = Command::new(&binary);
            if owner {
                command.arg("owner");
            }
            let output = command
                .env("RINZU_MANAGED_LOOKUP_CACHE", lookup)
                .env("RINZU_GC_STRESS", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{mode}/{owner}: {output:?}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
