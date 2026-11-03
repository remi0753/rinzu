#[cfg(unix)]
#[test]
fn c_metadata_reuse_preserves_identity_ownership_errors_and_gc_pacing() {
    use std::path::Path;
    use std::process::Command;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::env::temp_dir().join(format!("rinzu-metadata-cache-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let mut expected_pacing = None;
    for capacity in [0, 256, 1024, 2048, 4096] {
        let binary = work.join(format!("capacity-{capacity}"));
        let built = Command::new("clang")
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(root.join("include"))
            .arg(format!(
                "-DRINZU_MANAGED_METADATA_CACHE_CAPACITY={capacity}"
            ))
            .arg(root.join("crates/runtime/tests/fixtures/managed_metadata_cache.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(built.status.success(), "{built:?}");
        for (mode, expected) in [
            ("normal", 0),
            ("normal-off", 0),
            ("stale", 70),
            ("owner", 70),
            ("domain", 70),
            ("oom", 71),
            ("identity", 70),
        ] {
            let output = Command::new(&binary).arg(mode).output().unwrap();
            assert_eq!(output.status.code(), Some(expected), "{output:?}");
            if expected == 0 {
                let text = String::from_utf8(output.stdout).unwrap();
                let pacing = text.lines().find(|line| line.starts_with("PACE:")).unwrap();
                if let Some(ref previous) = expected_pacing {
                    assert_eq!(previous, pacing, "capacity={capacity}, mode={mode}");
                } else {
                    expected_pacing = Some(pacing.to_owned());
                }
            }
        }
    }
    std::fs::remove_dir_all(work).unwrap();
}
