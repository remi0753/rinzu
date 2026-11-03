#![cfg(unix)]
use std::process::Command;

#[test]
fn synchronous_writer_preserves_bytes_records_fd_flags_and_signals() {
    let root = std::env::temp_dir().join(format!("rinzu-stdio-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "normal" });
        let mut clang = Command::new("clang");
        clang
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/stdio.c"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("PASS stdout"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
    }
    std::fs::remove_dir_all(root).unwrap();
}
