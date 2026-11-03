#![cfg(unix)]
use std::path::Path;
use std::process::Command;

#[test]
fn c_loans_share_aliases_pin_gc_commit_atomically_and_enforce_fatal_phases() {
    let root = std::env::temp_dir().join(format!("rinzu-c-record-loans-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitize in [false, true] {
        let binary = root.join(if sanitize { "sanitized" } else { "normal" });
        let mut command = Command::new("clang");
        command
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/native_record_loans.c"));
        if sanitize {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = command.arg("-o").arg(&binary).output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for mode in [
            "normal",
            "discard",
            "invalid-bool",
            "early-release",
            "early-commit",
            "dispose-entered",
            "wrong-type",
            "wrong-domain",
            "alias-layout",
            "copied-layout",
            "null-alias-layout",
            "oom",
        ] {
            let run = Command::new(&binary).arg(mode).output().unwrap();
            let expected = match mode {
                "normal" | "discard" | "copied-layout" => 0,
                "oom" => 71,
                _ => 86,
            };
            assert_eq!(
                run.status.code(),
                Some(expected),
                "sanitize={sanitize}, {mode}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
            assert!(
                (expected == 0 && run.stderr.is_empty())
                    || (expected != 0
                        && String::from_utf8_lossy(&run.stderr).contains(if expected == 71 {
                            "resource unavailable (71)"
                        } else {
                            "contract violation (86)"
                        })),
                "{mode}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
            if expected == 0 {
                assert!(
                    String::from_utf8_lossy(&run.stdout)
                        .contains("PASS shared native record loans")
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
