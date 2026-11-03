#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_dispatcher_sdk/setup.rs"]
mod fixture;
fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}
fn baseline(output: &Output) {
    success(output);
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("RINZU_NATIVE_CALLS=7,7,7,7,7,7,1,0,0,0,0"),
        "{stdout}"
    );
    assert!(stdout.contains("RINZU_FOREIGN=1,1,1,1,0,0,0,0"), "{stdout}");
    assert!(
        stdout.contains("RINZU_STARTED_CHILDREN=2,2")
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
fn main_continuations_wake_inside_supplant_on_jit_aot_signed_and_sanitized_paths() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-dispatcher-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("fixtures/native_dispatcher_sdk/main.rinzu")
            .replace("dispatcher.reset(0)", "dispatcher.reset(1)"),
    )
    .unwrap();
    success(
        &Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .output()
            .unwrap(),
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
    );
    success(
        &Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .output()
            .unwrap(),
    );
    std::fs::remove_dir_all(root).unwrap();
}
