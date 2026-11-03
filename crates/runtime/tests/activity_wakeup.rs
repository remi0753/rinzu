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
fn activity_notifications_keep_idle_wake_active_ticks_foreign_publication_and_shutdown() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        Scratch(std::env::temp_dir().join(format!("rinzu-activity-wakeup-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let mut variants = vec![
        ("optimized", vec!["-O2"]),
        (
            "address-undefined",
            vec![
                "-O1",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
        ),
    ];
    if cfg!(target_os = "macos") {
        variants.push(("thread", vec!["-O1", "-g", "-fsanitize=thread"]));
    }
    for (name, flags) in variants {
        let binary = directory.0.join(name);
        let built = Command::new("clang")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .args(flags)
            .arg("-I")
            .arg(root.join("include"))
            .arg(root.join("crates/runtime/tests/fixtures/activity_wakeup.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(built.status.success(), "{name}: {built:?}");
        let result = Command::new(&binary)
            .arg("coalesced")
            .env_remove("RINZU_DIAGNOSTIC_OUTPUT")
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{name}: {result:?}");
        assert!(String::from_utf8_lossy(&result.stdout).contains("PASS activation transitions"));
        let warnings = String::from_utf8_lossy(&result.stderr);
        for symbol in ["already.warned", "later.deadline", "foreign.ready"] {
            assert_eq!(
                warnings.matches(&format!("symbol={symbol} ")).count(),
                1,
                "{warnings}"
            );
        }
    }
}
