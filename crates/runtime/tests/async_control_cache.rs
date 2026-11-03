#![cfg(unix)]
use std::path::Path;
use std::process::Command;

#[test]
fn dead_control_blocks_preserve_bounds_zeroing_identity_and_cleanup() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        std::env::temp_dir().join(format!("rinzu-control-cache-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    for sanitized in [false, true] {
        let binary = directory.join(if sanitized { "san" } else { "plain" });
        let mut build = Command::new("clang");
        build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            build.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = build
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/async_control_cache.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for mode in [
            None,
            Some("cold-oom"),
            Some("busy-dispose"),
            Some("foreign-owner"),
            Some("isolated-oom"),
        ] {
            let mut command = Command::new(&binary);
            if let Some(mode) = mode {
                command.arg(mode);
            }
            let output = command
                .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(match mode {
                    Some(mode) if mode.contains("oom") => 71,
                    Some(_) => 70,
                    None => 0,
                }),
                "{mode:?}: {output:?}"
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
