#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn child_handoff_preserves_all_sorted_partitions_records_errors_and_owner_guards() {
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("rinzu-child-handoff-{}", std::process::id()));
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
            .arg(root.join("crates/runtime/tests/fixtures/child_handoff_search.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        for argument in [
            None,
            Some("0"),
            Some("1"),
            Some("2"),
            Some("owner"),
            Some("oom"),
        ] {
            let mut command = Command::new(&binary);
            if let Some(argument) = argument {
                command.arg(argument);
            }
            let output = command
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
                Some(match argument {
                    Some("oom") => 71,
                    Some(_) => 70,
                    None => 0,
                }),
                "{mode}/{argument:?}: {output:?}"
            );
            if argument == Some("oom") {
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("resource unavailable (71)")
                );
            } else {
                assert!(output.stderr.is_empty(), "{mode}/{argument:?}: {output:?}");
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
