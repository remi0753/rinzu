#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../../sdk/io/tests/package.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
mod platform;
use std::process::Command;

#[test]
fn ordinary_io_import_uses_generated_services_without_metadata_or_native_shim() {
    let root = std::env::temp_dir().join(format!("rinzu-io-source-cli-{}", std::process::id()));
    let app = fixture::create(&root);
    platform::success(&platform::bounded(
        Command::new(frozen_cli::path()).arg("check").arg(&app),
    ));
    let relative = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg("app")
            .current_dir(&root),
    );
    platform::success(&relative);
    assert!(
        String::from_utf8_lossy(&relative.stderr).contains("compiled JIT continuation runtime")
    );
    platform::success(&platform::bounded(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/io_source_app");
    let check = |output: std::process::Output| {
        platform::success(&output);
        let stdout = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("RINZU_STARTED_TIMERS=1")
                && stdout.contains("RINZU_STARTED_CHILDREN=0,1"),
            "{stdout}"
        );
        assert!(
            stdout.contains("RINZU_CAUGHT_ERRORS=5@")
                && stdout.contains("\"resources\":\"baseline\""),
            "{stdout}"
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
    };
    check(platform::bounded(&mut Command::new(&binary)));
    let sanitized = platform::sanitize(&binary);
    check(platform::bounded(
        Command::new(&sanitized)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    check(platform::bounded(&mut Command::new(&binary)));
    platform::success(&platform::bounded(
        Command::new(frozen_cli::path()).arg("verify-aot").arg(&app),
    ));
    std::fs::remove_dir_all(root).unwrap();
}
