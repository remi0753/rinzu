#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
#[allow(dead_code)]
mod platform;
use std::process::Command;

#[test]
fn timer_library_runs_without_a_manifest_or_native_sdk_in_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-timer-library-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("timer.rinzu");
    std::fs::write(&source, include_str!("../../../sdk/timer/tests/main.rinzu")).unwrap();
    for action in ["check", "run", "verify-aot"] {
        let output = platform::bounded(Command::new(frozen_cli::path()).arg(action).arg(&source));
        assert!(output.status.success(), "{action}: {output:?}");
        if action == "run" {
            assert_eq!(output.stdout, b"timer library ok\n");
        }
    }
    assert!(!root.join("rinzu.toml").exists());
    std::fs::remove_dir_all(root).unwrap();
}
