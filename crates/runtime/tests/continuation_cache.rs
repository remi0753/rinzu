#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn continuation_cache_preserves_parked_frames_guards_owner_and_shutdown() {
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::process::Command;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary =
        std::env::temp_dir().join(format!("rinzu-continuation-cache-{}", std::process::id()));
    let output = Command::new("clang")
        .args([
            "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
        ])
        .arg(root.join("include"))
        .arg(root.join("crates/runtime/tests/fixtures/continuation_cache.c"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    for limit in [None, Some("0"), Some("600000")] {
        let mut run = Command::new(&binary);
        run.env_remove("RINZU_CONTINUATION_CACHE_BYTES");
        if let Some(limit) = limit {
            run.env("RINZU_CONTINUATION_CACHE_BYTES", limit);
        }
        let output = run.output().unwrap();
        assert!(output.status.success(), "{limit:?}: {output:?}");
    }
    for fault in ["live", "unfinished", "foreign", "double"] {
        let output = Command::new(&binary).arg(fault).output().unwrap();
        assert_eq!(output.status.code(), Some(70), "{fault}: {output:?}");
    }
    for guard in ["lower", "upper"] {
        let output = Command::new(&binary)
            .env_remove("RINZU_CONTINUATION_CACHE_BYTES")
            .arg(guard)
            .output()
            .unwrap();
        assert!(
            matches!(output.status.signal(), Some(10 | 11)),
            "{guard}: {output:?}"
        );
    }
    std::fs::remove_file(binary).unwrap();
}
