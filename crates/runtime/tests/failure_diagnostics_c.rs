#![cfg(unix)]
use std::process::Command;

#[test]
fn c_diagnostic_order_preserves_records_and_rejects_corruption_and_oom() {
    let root = std::env::temp_dir().join(format!("rinzu-failure-report-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "san" } else { "plain" });
        let mut compiler = Command::new("clang");
        compiler.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"]);
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = compiler
            .arg(manifest.join("tests/fixtures/failure_diagnostics.c"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let run = |mode: Option<&str>| {
            let mut command = Command::new(&binary);
            command.env("ASAN_OPTIONS", "detect_leaks=0");
            if let Some(mode) = mode {
                command.arg(mode);
            }
            command.output().unwrap()
        };
        let normal = run(None);
        assert!(normal.status.success(), "{normal:?}");
        assert!(normal.stderr.is_empty(), "{normal:?}");
        assert_eq!(
            String::from_utf8(normal.stdout).unwrap(),
            concat!(
                "RINZU_DIAGNOSTICS=0\nRINZU_CAUGHT_ERRORS=\n",
                "RINZU_DIAGNOSTICS=0\nRINZU_CAUGHT_ERRORS=5@1:10:11,12@1:20:21;15@2:30:31\n",
                "RINZU_DIAGNOSTICS=0\nRINZU_CAUGHT_ERRORS=5@1:10:11,12@1:20:21;15@2:30:31\n"
            )
        );
        for mode in [
            "missing",
            "extra",
            "stray",
            "duplicate",
            "order",
            "overflow",
            "oom",
        ] {
            let invalid = run(Some(mode));
            assert_eq!(
                invalid.status.code(),
                Some(if mode.contains("oom") { 71 } else { 70 }),
                "{mode}: {invalid:?}"
            );
            if mode == "oom" {
                assert!(
                    String::from_utf8_lossy(&invalid.stderr).contains("resource unavailable (71)")
                );
            } else {
                assert!(invalid.stderr.is_empty(), "{mode}: {invalid:?}");
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
