#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
#[allow(dead_code)] // Shared bounded runner also defines signing/sanitizer helpers.
mod platform;
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use scratch::Scratch;
use std::path::Path;
use std::process::Command;

#[test]
fn macos_source_packages_use_existing_language_and_native_boundaries() {
    const CHILD: &str = "RINZU_MACOS_FRONTEND_CHILD";
    const PACKAGES: &[&str] = &[
        "sdk/macos/tests/app",
        "sdk/macos/tests/gui",
        "sdk/macos/tests/graphics",
        "sdk/macos/tests/canvas",
        "sdk/macos/tests/data",
        "sdk/macos/tests/data-acquire",
        "sdk/macos/tests/bundle",
    ];
    if let Ok(package) = std::env::var(CHILD) {
        assert!(PACKAGES.contains(&package.as_str()));
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        rinzu_compiler::check_package(&root.join(&package))
            .unwrap_or_else(|diagnostics| panic!("{package}: {diagnostics:#?}"));
        return;
    }
    let cache = Scratch::new("rinzu-macos-frontend");
    // Share a fresh cache and give each package its own compiler deadline.
    let executable = std::env::current_exe().unwrap();
    for package in PACKAGES {
        let result = platform::bounded_tool(
            Command::new(&executable)
                .args([
                    "--exact",
                    "macos_source_packages_use_existing_language_and_native_boundaries",
                    "--nocapture",
                ])
                .env(CHILD, package)
                .env("RINZU_CACHE_DIR", cache.path()),
        );
        assert!(result.status.success(), "{package}: {result:?}");
    }
}

fn check_native_cases(script: &str, names: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scratch = Scratch::new("rinzu-macos-sdk");
    let output = scratch.path().join("report.json");
    let result = platform::bounded_tool(
        Command::new("python3")
            .arg(root.join("tests/integration").join(script))
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
    let cases = report["cases"].as_array().unwrap();
    for name in names {
        assert!(
            cases
                .iter()
                .any(|case| case["name"] == *name && case["passed"] == true),
            "{name}: {report}"
        );
    }
}

#[test]
fn macos_gui_stream_widgets_menus_and_owned_snapshots_are_reproducible() {
    check_native_cases(
        "test_macos_gui.py",
        &[
            "native-stream",
            "native-overflow",
            "native-widgets",
            "native-menus",
        ],
    );
}

#[test]
fn macos_async_sheets_and_idle_cancellation_are_reproducible() {
    check_native_cases(
        "test_macos_sheets.py",
        &[
            "native-pre-attach",
            "native-cancel-idle",
            "native-two",
            "native-external-stop",
            "native-state",
        ],
    );
}

#[test]
fn macos_native_owner_resource_and_available_gpu_checks_are_reproducible() {
    check_native_cases(
        "test_macos_sdk.py",
        &[
            "native-core",
            "native-data-model",
            "native-data-tree",
            "native-resources",
            "native-text-measure",
        ],
    );
}
