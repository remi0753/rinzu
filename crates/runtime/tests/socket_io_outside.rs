#![cfg(target_os = "macos")]
use std::{path::Path, process::Command};

#[test]
fn sdk_pending_io_keeps_ownership_while_other_calls_progress() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::env::temp_dir().join(format!("rinzu-socket-io-outside-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    for sanitizer in ["none", "address,undefined", "thread"] {
        let binary = work.join("io-outside");
        let mut command = Command::new("clang");
        command
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(repo.join("include"))
            .arg("-I")
            .arg(repo.join("sdk/socket/native/include"))
            .arg(repo.join("sdk/socket/tests/io-outside.c"));
        if sanitizer != "none" {
            command
                .arg(format!("-fsanitize={sanitizer}"))
                .arg("-fno-sanitize-recover=all");
        }
        let built = command.arg("-o").arg(&binary).output().unwrap();
        assert!(built.status.success(), "{sanitizer}: {built:?}");
        for guard in [
            None,
            Some("pending-finish"),
            Some("0"),
            Some("1"),
            Some("2"),
            Some("3"),
            Some("4"),
            Some("5"),
        ] {
            let mut run = Command::new(&binary);
            if let Some(guard) = guard {
                run.arg(guard);
            }
            let result = run
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .env("TSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                result.status.code(),
                Some(if guard.is_some() { 86 } else { 0 }),
                "{sanitizer} {guard:?}: {result:?}"
            );
            if guard.is_none() {
                assert!(
                    String::from_utf8_lossy(&result.stdout)
                        .contains("io-outside:48:pending-lease:")
                );
            }
            let errors = String::from_utf8_lossy(&result.stderr);
            assert!(
                !errors.contains("Sanitizer") && !errors.contains("runtime error:"),
                "{result:?}"
            );
        }
    }
    std::fs::remove_dir_all(work).unwrap();
}
