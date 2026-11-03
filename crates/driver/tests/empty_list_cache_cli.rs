#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn empty_lists_keep_aliases_workers_gc_and_constructor_quota() {
    use std::{path::Path, process::Command};
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(fixtures.join("empty_list_cache.rinzu"))
        .env("RINZU_GC_STRESS", "1")
        .env("RINZU_ERROR_TRACE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    for stress in ["0", "1"] {
        let output = Command::new(frozen_cli::path())
            .args(["verify-aot"])
            .arg(fixtures.join("empty_list_quota.rinzu"))
            .args(["--max-allocations", "16"])
            .env("RINZU_GC_STRESS", stress)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=overloaded"));
    }
}
