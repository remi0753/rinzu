#![cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn error_encoding_grows_amortized_and_keeps_self_appends_graphs_and_overflow_guards() {
    let directory =
        Scratch(std::env::temp_dir().join(format!("rinzu-error-buffer-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for sanitized in [false, true] {
        let binary = directory.0.join("probe");
        let mut build = Command::new("clang");
        build.args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
            "-DRINZU_EXPECT_AMORTIZED_ERROR_BUFFER=1",
        ]);
        if sanitized {
            build.args([
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ]);
        }
        let result = build
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/examples/error_buffer_probe.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let result = Command::new(&binary)
            .arg("error")
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(String::from_utf8(result.stdout).unwrap().lines().count(), 3);
        let result = Command::new(&binary)
            .arg("overflow")
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(70), "{result:?}");
    }
}
