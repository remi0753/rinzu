#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
#[allow(dead_code)] // Shared bounded runner also defines signing/sanitizer helpers.
mod platform;
use std::path::Path;
use std::process::Command;

#[test]
fn macos_source_packages_use_existing_language_and_native_boundaries() {
    const CHILD: &str = "RINZU_MACOS_FRONTEND_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let cache = std::env::temp_dir().join(format!(
            "rinzu-macos-frontend-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&cache).unwrap();
        // This child checks fourteen packages and populates a cold SDK cache.
        // Use the tool deadline rather than the individual runtime deadline.
        let result = platform::bounded_tool(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "macos_source_packages_use_existing_language_and_native_boundaries",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("RINZU_CACHE_DIR", &cache),
        );
        std::fs::remove_dir_all(cache).unwrap();
        platform::success(&result);
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for package in [
        "sdk/macos/tests/app",
        "sdk/macos/tests/gui",
        "sdk/macos/tests/graphics",
        "sdk/macos/tests/canvas",
        "sdk/macos/tests/data",
        "sdk/macos/tests/data-acquire",
        "sdk/macos/tests/bundle",
        "tests/fixtures/programs/macos-metal",
        "tests/fixtures/programs/macos-widgets",
        "tests/fixtures/programs/macos-notepad",
        "tests/fixtures/programs/macos-image-viewer",
        "tests/fixtures/programs/macos-charts",
        "tests/fixtures/programs/macos-data-browser",
        "tests/fixtures/programs/macos-task-list",
    ] {
        rinzu_compiler::check_package(&root.join(package))
            .unwrap_or_else(|diagnostics| panic!("{package}: {diagnostics:#?}"));
    }
}

#[test]
fn macos_gui_stream_widgets_menus_and_owned_snapshots_are_reproducible() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = std::env::temp_dir().join(format!("rinzu-macos-gui-{}.json", std::process::id()));
    let result = platform::bounded(
        Command::new("python3")
            .arg(root.join("tests/integration/test_macos_gui.py"))
            .arg("--rinzu")
            .arg(frozen_cli::path())
            .arg("--native-only")
            .arg("--output")
            .arg(&output),
    );
    platform::success(&result);
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["passed"], true, "{report}");
    for name in [
        "native-stream",
        "native-overflow",
        "native-widgets",
        "native-menus",
    ] {
        assert!(
            report["cases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|case| { case["name"] == name && case["passed"] == true }),
            "{name}: {report}"
        );
    }
    std::fs::remove_file(output).unwrap();
}

#[test]
fn macos_async_sheets_and_idle_cancellation_are_reproducible() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output =
        std::env::temp_dir().join(format!("rinzu-macos-sheets-{}.json", std::process::id()));
    let result = platform::bounded(
        Command::new("python3")
            .arg(root.join("tests/integration/test_macos_sheets.py"))
            .arg("--rinzu")
            .arg(frozen_cli::path())
            .arg("--native-only")
            .arg("--output")
            .arg(&output),
    );
    platform::success(&result);
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["passed"], true, "{report}");
    for name in [
        "native-pre-attach",
        "native-cancel-idle",
        "native-two",
        "native-external-stop",
        "native-state",
    ] {
        assert!(
            report["cases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|case| case["name"] == name && case["passed"] == true),
            "{name}: {report}"
        );
    }
    std::fs::remove_file(output).unwrap();
}

#[test]
fn macos_native_owner_resource_and_available_gpu_checks_are_reproducible() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output =
        std::env::temp_dir().join(format!("rinzu-macos-native-{}.json", std::process::id()));
    let result = platform::bounded(
        Command::new("python3")
            .arg(root.join("tests/integration/test_macos_sdk.py"))
            .arg("--rinzu")
            .arg(frozen_cli::path())
            .arg("--native-only")
            .arg("--output")
            .arg(&output),
    );
    platform::success(&result);
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["passed"], true, "{report}");
    assert!(
        report["cases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|case| { case["name"] == "native-core" && case["passed"] == true })
    );
    std::fs::remove_file(output).unwrap();
}
