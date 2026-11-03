#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_dispatcher_sdk/diagnostics_source.rs"]
mod diagnostic_source;
#[path = "fixtures/native_dispatcher_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}
fn warnings(output: &Output, variant: &str) {
    success(output);
    let warnings = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "long-native symbol=dispatcher_run ",
        "long-native symbol=dispatcher_delay ",
        "long-native symbol=dispatcher_worker_delay ",
        "long-managed symbol=callback ",
        "long-managed symbol=flow ",
        "long-managed symbol=calculate ",
        "foreign-result-wait symbol=callback ",
    ] {
        assert!(
            warnings.contains(expected),
            "missing {expected}: {warnings}"
        );
    }
    for warning in warnings
        .lines()
        .filter(|line| line.starts_with("rinzu warning:"))
    {
        assert!(!warning.contains("source=0:"), "{warning}");
        assert!(
            warning.contains("owner=0x")
                && warning.contains("depth=")
                && warning.contains("elapsed_ms=")
                && warning.contains("stack="),
            "{warning}"
        );
    }
    assert!(!warnings.contains("runtime error:"));
    if variant == "packet" {
        let warning = warnings
            .lines()
            .find(|line| line.contains("owner-stall symbol=reactor.file-read-size "))
            .unwrap_or_else(|| panic!("{warnings}"));
        assert!(
            warning.contains("dispatcher_delay") && warning.contains("callback"),
            "{warning}"
        );
    } else if variant == "main" {
        let warning = warnings
            .lines()
            .find(|line| line.contains("owner-stall symbol=observeReady "))
            .unwrap_or_else(|| panic!("{warnings}"));
        assert!(
            warning.contains("dispatcher_delay") && warning.contains("holdReady"),
            "{warning}"
        );
        assert!(
            !warnings.contains("owner-stall symbol=reactor.file-read-size "),
            "{warnings}"
        );
    } else {
        assert!(!warnings.contains("owner-stall "), "{warnings}");
    }
}
fn baseline(output: &Output, variant: &str) {
    warnings(output, variant);
    let stdout = String::from_utf8_lossy(&output.stderr);
    let calls = if variant == "main" { 12 } else { 11 };
    assert!(
        stdout.contains(&format!(
            "RINZU_NATIVE_CALLS={calls},{calls},{calls},{calls},{calls},{calls},1,0,0,0,0"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("RINZU_FOREIGN=1,1,1,1,0,0,0,0"), "{stdout}");
    assert!(
        stdout.contains("RINZU_STARTED_CHILDREN=2,3")
            && stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    assert!(
        stdout.find("DISPATCHER_FLOW_DONE_BEFORE_RETURN").unwrap()
            < stdout.find("DISPATCHER_NATIVE_RETURN").unwrap(),
        "{stdout}"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
}
#[test]
fn actual_native_and_managed_activations_warn_without_interrupting_jit_aot_signed_or_sanitized_paths()
 {
    for variant in ["normal", "packet", "main"] {
        verify_fixture(variant);
    }
}
fn verify_fixture(variant: &str) {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-native-diagnostics-{}-{variant}",
        std::process::id(),
    ));
    fixture::create(&root);
    let app = root.join("app");
    let source = match variant {
        "main" => diagnostic_source::main_ready_source(1),
        "packet" => {
            diagnostic_source::source(1).replace("dispatcher.delay(120)", "dispatcher.delay(1200)")
        }
        _ => diagnostic_source::source(1),
    };
    std::fs::write(app.join("src/main.rinzu"), source).unwrap();
    warnings(
        &Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .output()
            .unwrap(),
        variant,
    );
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    let binary = app.join("build/dispatcher_app");
    baseline(
        &Command::new(&binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap(),
        variant,
    );
    let entitlements = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlements,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    success(
        &Command::new("codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--options",
                "runtime",
                "--entitlements",
            ])
            .arg(&entitlements)
            .arg(&binary)
            .output()
            .unwrap(),
    );
    success(
        &Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&binary)
            .output()
            .unwrap(),
    );
    let actual = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(&binary)
        .output()
        .unwrap();
    success(&actual);
    let actual = String::from_utf8_lossy(&actual.stderr);
    assert!(!actual.contains("allow-jit") && !actual.contains("allow-unsigned-executable-memory"));
    baseline(
        &Command::new(&binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap(),
        variant,
    );
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sanitized = app.join("build/dispatcher-sanitized");
    success(
        &Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pthread",
                "-fsanitize=address,undefined",
                "-fno-omit-frame-pointer",
            ])
            .arg(app.join("build/dispatcher_app.o"))
            .arg(app.join("build/dispatcher_app.runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg(manifest.join("../runtime/src/owner_dispatcher.c"))
            .args(["-framework", "CoreFoundation"])
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&sanitized)
            .output()
            .unwrap(),
    );
    std::os::unix::fs::symlink(
        app.join("build/dispatcher_app.resources"),
        sanitized.with_extension("resources"),
    )
    .unwrap();
    baseline(
        &Command::new(sanitized)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap(),
        variant,
    );
    let verified = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&app)
        .output()
        .unwrap();
    // The isolated verifier keeps timing diagnostics separate from program
    // stderr. The executions above validate the actual warnings on each path.
    success(&verified);
    assert!(String::from_utf8_lossy(&verified.stdout).contains("JIT/AOT equivalent:"));
    std::fs::remove_dir_all(root).unwrap();
}
