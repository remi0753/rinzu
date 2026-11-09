#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_termination_sdk/assertions.rs"]
mod assertions;
#[path = "fixtures/native_termination_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/native_termination_sdk/interrupt.rs"]
mod interrupt;
#[path = "fixtures/native_pump_sdk/assertions.rs"]
mod pump_assertions;
#[path = "fixtures/native_pump_sdk/setup.rs"]
mod pump_fixture;

#[test]
fn shutdown_blocker_keeps_owner_pump_for_real_main_cleanup_stop_and_resource_settlement() {
    let root = std::env::temp_dir().join(format!("rinzu-owner-pump-cli-{}", std::process::id()));
    pump_fixture::create(&root);
    let app = root.join("app");
    success(&cli(&app, "build"));
    let aot = app.join("build/pump_app");
    let sanitized = app.join("build/pump-sanitized");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
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
            .arg(aot.with_extension("o"))
            .arg(aot.with_extension("runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&sanitized)
            .output()
            .unwrap(),
    );
    std::os::unix::fs::symlink(
        aot.with_extension("source-packages"),
        sanitized.with_extension("source-packages"),
    )
    .unwrap();
    let check = |output: Output, failure: &str| {
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains(&format!("RINZU_FAILURE={failure}"))
                && stdout.contains("\"resources\":\"baseline\""),
            "{stdout}"
        );
        assert!(
            stdout.contains("RINZU_NATIVE_CALLS=7,7,7,7,7,7,0,0,0,0,0")
                && stdout.contains("RINZU_STARTED_CHILDREN=2,0"),
            "{stdout}"
        );
        pump_assertions::progress(&String::from_utf8_lossy(&output.stderr), 1);
    };
    for (variable, failure) in [
        ("RINZU_CANCEL_WORKER_AFTER_POLLS", "cancelled"),
        ("RINZU_SHUTDOWN_WORKER_AFTER_POLLS", "shutdown"),
    ] {
        for binary in [&aot, &sanitized] {
            check(
                Command::new(binary)
                    .env(variable, "80")
                    .env("RINZU_TRANSPORT_EVIDENCE", "1")
                    .env("ASAN_OPTIONS", "detect_leaks=0")
                    .env("UBSAN_OPTIONS", "halt_on_error=1")
                    .output()
                    .unwrap(),
                failure,
            );
        }
    }
    sign(&aot, &root);
    for (variable, failure) in [
        ("RINZU_CANCEL_WORKER_AFTER_POLLS", "cancelled"),
        ("RINZU_SHUTDOWN_WORKER_AFTER_POLLS", "shutdown"),
    ] {
        check(
            Command::new(&aot)
                .env(variable, "80")
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap(),
            failure,
        );
    }
    success(
        &Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .args(["--cancel-worker-after-polls", "80"])
            .output()
            .unwrap(),
    );
    std::fs::remove_dir_all(root).unwrap();
}

fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}
fn cli(app: &Path, action: &str) -> Output {
    Command::new(frozen_cli::path())
        .args([action])
        .arg(app)
        .output()
        .unwrap()
}
fn sign(binary: &Path, root: &Path) {
    let entitlement = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlement, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
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
            .arg(entitlement)
            .arg(binary)
            .output()
            .unwrap(),
    );
    success(
        &Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(binary)
            .output()
            .unwrap(),
    );
}

#[test]
fn permanent_native_main_supplant_worker_stop_on_one_interrupt_with_inherited_signal_policy() {
    let root = std::env::temp_dir().join(format!("rinzu-interrupt-cli-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    for place in ["supplant", "main", "worker"] {
        std::fs::write(app.join("src/main.rinzu"), interrupt::source(place)).unwrap();
        success(&cli(&app, "build"));
        let aot = app.join("build/termination_app");
        let sanitized = app.join("build/interrupt-sanitized");
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
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
                .arg(aot.with_extension("o"))
                .arg(aot.with_extension("runtime.c"))
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
        let bundle = sanitized.with_extension("source-packages");
        if bundle.symlink_metadata().is_err() {
            std::os::unix::fs::symlink(aot.with_extension("source-packages"), &bundle).unwrap();
        }
        let resources = sanitized.with_extension("resources");
        if resources.symlink_metadata().is_err() {
            std::os::unix::fs::symlink(aot.with_extension("resources"), &resources).unwrap();
        }
        for policy in 0..3 {
            let mut command = Command::new(frozen_cli::path());
            command
                .arg("run")
                .arg(&app)
                .env("RINZU_INTERRUPT_PROCESS_MAIN", "1");
            interrupt::terminate_once(&mut command, policy, place == "supplant");
            for binary in [&aot, &sanitized] {
                let mut command = Command::new(binary);
                command
                    .env("RINZU_INTERRUPT_PROCESS_MAIN", "1")
                    .env("ASAN_OPTIONS", "detect_leaks=0");
                interrupt::terminate_once(&mut command, policy, place == "supplant");
            }
        }
        sign(&aot, &root);
        for policy in 0..3 {
            let mut command = Command::new(&aot);
            command.env("RINZU_INTERRUPT_PROCESS_MAIN", "1");
            interrupt::terminate_once(&mut command, policy, place == "supplant");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn callback_fail_fast_and_explicit_fallbacks_match_compiled_jit_aot_signed_and_sanitizers() {
    let root = std::env::temp_dir().join(format!("rinzu-termination-cli-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let api_path = root.join("termination/src/api.rinzu");
    let api = std::fs::read_to_string(&api_path).unwrap();
    for invalid in [
        api.replacen(
            "rinzu.core.panic(\"invalid\");",
            "let invalid = CInt8(300);",
            1,
        ),
        api.replacen(
            "fn missingUnit() -> Unit {",
            "fn missingUnit() -> Unit { let rejectedNormalError=CInt8(300);",
            1,
        ),
        api.replacen(
            "fn missingText(value: String) -> CInt fallback 31",
            "fn missingText(value: String) -> CInt",
            1,
        ),
    ] {
        std::fs::write(&api_path, invalid).unwrap();
        let rejected = cli(&app, "check");
        assert!(!rejected.status.success());
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("RZ0413"),
            "{rejected:?}"
        );
    }
    std::fs::write(&api_path, api).unwrap();
    success(&cli(&app, "build"));
    let aot = app.join("build/termination_app");
    let sanitized = app.join("build/termination-sanitized");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
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
            .arg(aot.with_extension("o"))
            .arg(aot.with_extension("runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&sanitized)
            .output()
            .unwrap(),
    );
    std::os::unix::fs::symlink(
        aot.with_extension("source-packages"),
        sanitized.with_extension("source-packages"),
    )
    .unwrap();
    for mode in 0..9 {
        let run = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .env("RINZU_TERMINATION_MODE", mode.to_string())
            .output()
            .unwrap();
        assertions::boundary(&run, mode);
        for binary in [&aot, &sanitized] {
            let output = Command::new(binary)
                .env("RINZU_TERMINATION_MODE", mode.to_string())
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assertions::boundary(&output, mode);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
        }
    }
    sign(&aot, &root);
    for mode in 0..9 {
        assertions::boundary(
            &Command::new(&aot)
                .env("RINZU_TERMINATION_MODE", mode.to_string())
                .output()
                .unwrap(),
            mode,
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
