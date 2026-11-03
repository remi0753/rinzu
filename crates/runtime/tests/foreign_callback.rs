#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;

#[test]
fn actual_foreign_entry_checks_progress_and_preserves_admission_through_receipt() {
    let root = std::env::temp_dir().join(format!("rinzu-foreign-callback-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11",
                "-O2",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pthread",
                "-DRINZU_CALLBACK_TESTING",
            ])
            .arg(manifest.join("tests/fixtures/foreign_callback.c"))
            .arg(manifest.join("src/callback_entry.c"))
            .arg(manifest.join("src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"));
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
        let success = run("normal");
        assert!(success.status.success(), "{success:?}");
        assert!(
            String::from_utf8_lossy(&success.stdout)
                .contains("PASS foreign callback signature/progress")
        );
        let diagnostic = run("diagnostics");
        assert!(diagnostic.status.success(), "{diagnostic:?}");
        let warnings = String::from_utf8_lossy(&diagnostic.stderr);
        assert_eq!(
            warnings
                .matches("foreign-result-wait symbol=fixture.callback source=17:31:49 ")
                .count(),
            1,
            "{warnings}"
        );
        let wait = warnings
            .lines()
            .find(|line| line.contains("foreign-result-wait "))
            .unwrap();
        assert!(wait.contains("stack=fixture.native"), "{warnings}");
        assert!(!warnings.contains("runtime error:"));
        let stalled = run("stall");
        assert!(stalled.status.success(), "{stalled:?}");
        let warnings = String::from_utf8_lossy(&stalled.stderr);
        let warning = warnings
            .lines()
            .find(|line| line.contains("owner-stall symbol=fixture.callback source=17:31:49 "))
            .unwrap_or_else(|| panic!("{warnings}"));
        assert!(warning.contains("stack=fixture.native"), "{warning}");
        assert_eq!(warnings.matches("owner-stall ").count(), 1, "{warnings}");
        let oom = run("diagnostic-oom");
        assert_eq!(oom.status.code(), Some(71), "{oom:?}");
        assert!(oom.stdout.is_empty() && oom.stderr.is_empty(), "{oom:?}");
        for mode in [
            "worker",
            "ordinary",
            "stale-generation",
            "bounded",
            "unsupported",
            "prepared",
            "closed",
            "foreign-bind",
            "duplicate-bind",
            "late-bind",
            "diagnostic-duplicate",
            "diagnostic-open",
            "diagnostic-foreign",
        ] {
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
