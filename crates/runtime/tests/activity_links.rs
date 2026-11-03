#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn constant_time_activity_unlink_preserves_membership_order_and_warning_pins() {
    use std::{path::Path, process::Command};
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("rinzu-activity-links-{}", std::process::id()));
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
            .arg(root.join("crates/runtime/tests/fixtures/activity_links.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        for guard in [
            None,
            Some("unknown"),
            Some("bad-link"),
            Some("double-end"),
            Some("linked-release"),
        ] {
            let mut command = Command::new(&binary);
            if let Some(value) = guard {
                command.arg(value);
            }
            let output = command
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if guard.is_some() { 86 } else { 0 }),
                "{mode}/{guard:?}: {output:?}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
