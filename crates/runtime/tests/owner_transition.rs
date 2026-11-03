#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;

#[test]
fn c_owner_frames_restore_nested_publications_and_reject_invalid_entries() {
    let root = std::env::temp_dir().join(format!("rinzu-owner-transition-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .arg(manifest.join("tests/fixtures/owner_transition.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let output = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !sanitized {
            for args in [
                vec!["--force", "--sign", "-", "--options", "runtime"],
                vec!["--verify", "--strict"],
            ] {
                let output = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        let output = Command::new(&binary).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("576 nested levels"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
        for mode in [
            "thread",
            "closed",
            "managed-callback",
            "overflow",
            "order",
            "stale",
            "callback-order",
            "finish",
        ] {
            let output = Command::new(&binary).arg(mode).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(86),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stdout.is_empty() && output.stderr.is_empty(),
                "{mode}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
