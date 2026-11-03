#![cfg(target_os = "macos")]
use std::{path::Path, process::Command};

#[test]
fn owner_write_preserves_worker_lease_and_completion_contracts() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work =
        std::env::temp_dir().join(format!("rinzu-socket-owner-write-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    for sanitizer in ["none", "address,undefined", "thread"] {
        let binary = work.join("owner-write");
        let mut command = Command::new("clang");
        command
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(repo.join("include"))
            .arg("-I")
            .arg(repo.join("sdk/socket/native/include"))
            .arg(repo.join("sdk/socket/tests/owner-write.c"));
        if sanitizer != "none" {
            command
                .arg(format!("-fsanitize={sanitizer}"))
                .arg("-fno-sanitize-recover=all");
        }
        let built = command.arg("-o").arg(&binary).output().unwrap();
        assert!(built.status.success(), "{sanitizer}: {built:?}");
        let result = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .env("TSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{sanitizer}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("owner-write:singlecall;snapshot;partial;transient;")
        );
        let errors = String::from_utf8_lossy(&result.stderr);
        assert!(
            !errors.contains("Sanitizer") && !errors.contains("runtime error:"),
            "{result:?}"
        );
    }
    std::fs::remove_dir_all(work).unwrap();
}
