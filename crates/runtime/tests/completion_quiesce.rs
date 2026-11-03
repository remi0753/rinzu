#![cfg(unix)]
use std::path::Path;
use std::process::Command;

#[test]
fn native_wake_target_outlives_publication_and_reactor_callback_return() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("rinzu-quiesce-{}", std::process::id()));
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
            .arg(root.join("crates/runtime/tests/fixtures/completion_quiesce.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for mode in [
            "normal",
            "notified",
            "late-start",
            "pending-helper",
            "pending-native",
        ] {
            let mut run = Command::new(&binary);
            if mode != "normal" {
                run.arg(mode);
            }
            let output = run
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode == "normal" || mode == "notified" {
                    0
                } else {
                    70
                }),
                "{mode} sanitized={sanitized}: {output:?}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
