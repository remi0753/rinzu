#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;

#[test]
fn runloop_source_coalesces_and_drains_with_bounds_and_progress_rejection() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = std::env::temp_dir().join(format!("rinzu-owner-dispatcher-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O2", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
            ])
            .arg("-DRINZU_DISPATCHER_TESTING")
            .arg(manifest.join("tests/fixtures/owner_dispatcher.c"))
            .arg(manifest.join("src/owner_dispatcher.c"))
            .arg(manifest.join("src/callback_entry.c"))
            .arg(manifest.join("src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .args(["-framework", "CoreFoundation"]);
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        if !sanitized {
            for args in [
                vec!["--force", "--sign", "-", "--options", "runtime"],
                vec!["--verify", "--strict"],
            ] {
                let signed = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(signed.status.success(), "{signed:?}");
            }
            let entitlements = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(entitlements.status.success() && entitlements.stdout.is_empty());
        }
        let run = |mode| {
            Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap()
        };
        for mode in ["normal", "common-mode", "budget", "lost-wake"] {
            let output = run(mode);
            assert!(output.status.success(), "{mode}: {output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("PASS runtime-owned RunLoop"));
        }
        for mode in ["bounded", "unknown-mode", "nested"] {
            let rejected = run(mode);
            assert_eq!(rejected.status.code(), Some(86), "{mode}: {rejected:?}");
            assert!(
                rejected.stdout.is_empty() && rejected.stderr.is_empty(),
                "{mode}: {rejected:?}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
