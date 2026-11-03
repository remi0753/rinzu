#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;
#[allow(dead_code)]
#[path = "support/source_platform.rs"]
mod platform;
#[test]
fn file_faults_cancel_admission_and_owner_settle_with_instrumented_native_sdk() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-io-fault-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for sanitizer in ["address,undefined", "thread"] {
        let binary = root.join(sanitizer.replace(",", "-"));
        platform::success(&platform::bounded(
            Command::new("clang")
                .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
                .arg(format!("-fsanitize={sanitizer}"))
                .arg("-I")
                .arg(repo.join("include"))
                .arg("-I")
                .arg(repo.join("sdk/io/native/include"))
                .arg(repo.join("sdk/io/tests/fault.c"))
                .arg("-o")
                .arg(&binary),
        ));
        let output = platform::bounded(
            Command::new(&binary)
                .current_dir(&root)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1"),
        );
        platform::success(&output);
        assert_eq!(output.stdout, b"io-fault-ok\n");
    }
    std::fs::remove_dir_all(root).unwrap();
}
