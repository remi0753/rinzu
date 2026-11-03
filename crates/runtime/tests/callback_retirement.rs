#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;

#[test]
fn retired_controls_keep_late_entries_distinct_with_unknowns_and_concurrent_owners() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let directory = std::env::temp_dir().join(format!("rinzu-retirement-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for mode in ["plain", "address,undefined", "thread"] {
        let binary = directory.join(mode);
        let mut command = Command::new("clang");
        command.args([
            "-std=c11", "-O1", "-g", "-pthread", "-Wall", "-Wextra", "-Werror",
        ]);
        if mode != "plain" {
            command.arg(format!("-fsanitize={mode}"));
        }
        let output = command
            .arg(root.join("tests/fixtures/callback_retire.c"))
            .arg(root.join("src/callback_entry.c"))
            .arg(root.join("src/callback_template.S"))
            .arg("-I")
            .arg(root.join("../../include"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for (argument, expected) in [
            ("normal", 0),
            ("late-first", 86),
            ("late-middle", 86),
            ("late-last", 86),
            ("late-unknown", 86),
            ("wrong-owner", 86),
            ("null-owner", 86),
        ] {
            let output = Command::new(&binary)
                .arg(argument)
                .env("ASAN_OPTIONS", "detect_leaks=0:halt_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{mode}/{argument}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stderr.is_empty(),
                "{mode}/{argument}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if expected == 0 {
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(
                    stdout.contains("entries=16385 retired=15873 unknown=512"),
                    "{stdout}"
                );
                eprintln!("{mode}: {stdout}");
            } else {
                assert!(output.stdout.is_empty());
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
