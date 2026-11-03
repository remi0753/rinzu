#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_foreign_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/native_foreign_sdk/return_probe.rs"]
mod return_probe;
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn baseline(output: &Output) {
    success(output);
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("RINZU_NATIVE_CALLS=3,3,3,3,3,3,48,576,0,0,0"),
        "{stdout}"
    );
    assert!(stdout.contains("RINZU_STARTED_CHILDREN=0,8"), "{stdout}");
    assert!(
        stdout.contains("RINZU_FOREIGN=72,72,72,72,0,0,0,0"),
        "{stdout}"
    );
    assert!(
        stdout.contains("RINZU_FAILURE=none") && stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    assert!(stdout.contains("FOREIGN_NATIVE_RETURN") && !stdout.contains("FOREIGN_REJECTED_BODY"));
}
fn sign(binary: &Path, root: &Path) {
    let entitlements = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
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
    let actual = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(binary)
        .output()
        .unwrap();
    success(&actual);
    let actual = String::from_utf8_lossy(&actual.stdout);
    assert!(
        actual.contains("disable-library-validation")
            && !actual.contains("allow-jit")
            && !actual.contains("allow-unsigned-executable-memory")
    );
}
fn sanitizer(app: &Path) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = app.join("build/foreign-sanitized");
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
            .arg(app.join("build/foreign_app.o"))
            .arg(app.join("build/foreign_app.runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg(manifest.join("../runtime/src/owner_dispatcher.c"))
            .args(["-framework", "CoreFoundation"])
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap(),
    );
    std::os::unix::fs::symlink(
        app.join("build/foreign_app.resources"),
        binary.with_file_name("foreign-sanitized.resources"),
    )
    .unwrap();
    binary
}
fn return_race_binary(app: &Path, sanitized: bool) -> PathBuf {
    let source = std::fs::read_to_string(app.join("build/foreign_app.runtime.c")).unwrap();
    let path = app.join("build/foreign-return-probe.c");
    std::fs::write(&path, return_probe::instrument(source, 8)).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = app.join(if sanitized {
        "build/foreign-return-sanitized"
    } else {
        "build/foreign-return-signed"
    });
    let mut command = Command::new("clang");
    command
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pthread",
            "-DRINZU_DISPATCHER_TESTING",
        ])
        .arg(app.join("build/foreign_app.o"))
        .arg(path)
        .arg(manifest.join("../runtime/src/callback_entry.c"))
        .arg(manifest.join("../runtime/src/callback_template.S"))
        .arg(manifest.join("../runtime/src/owner_dispatcher.c"))
        .args(["-framework", "CoreFoundation"])
        .arg("-I")
        .arg(manifest.join("../../include"));
    if sanitized {
        command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
    }
    success(&command.arg("-o").arg(&binary).output().unwrap());
    std::os::unix::fs::symlink(
        app.join("build/foreign_app.resources"),
        binary.with_extension("resources"),
    )
    .unwrap();
    binary
}
#[test]
fn foreign_sdk_runs_through_generated_source_package_jit_aot_signed_and_sanitizers() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-foreign-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let source = include_str!("fixtures/native_foreign_sdk/main.rinzu")
        .replace("foreign.reset(0)", "foreign.reset(1)");
    std::fs::write(app.join("src/main.rinzu"), source).unwrap();
    let run = Command::new(frozen_cli::path())
        .arg("run")
        .arg(&app)
        .output()
        .unwrap();
    success(&run);
    assert!(String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime"));
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    let binary = app.join("build/foreign_app");
    baseline(
        &Command::new(&binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap(),
    );
    sign(&binary, &root);
    baseline(
        &Command::new(&binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap(),
    );
    let sanitized = sanitizer(&app);
    let checked = Command::new(&sanitized)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1")
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    baseline(&checked);
    assert!(!String::from_utf8_lossy(&checked.stderr).contains("runtime error:"));
    success(
        &Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .output()
            .unwrap(),
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn foreign_cancellation_and_shutdown_wait_for_all_receipts_and_discard_protected_result() {
    for (name, variable, flag) in [
        (
            "cancelled",
            "RINZU_CANCEL_AFTER_POLLS",
            "--cancel-after-polls",
        ),
        (
            "shutdown",
            "RINZU_SHUTDOWN_AFTER_POLLS",
            "--shutdown-after-polls",
        ),
    ] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-cli-foreign-mask-{name}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        let app = root.join("app");
        std::fs::write(app.join("src/main.rinzu"), fixture::cancellation_source(1)).unwrap();
        success(
            &Command::new(frozen_cli::path())
                .arg("build")
                .arg(&app)
                .output()
                .unwrap(),
        );
        let run = |binary: &Path| {
            Command::new(binary)
                .env(variable, "80")
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap()
        };
        let check = |output: &Output| {
            assert_eq!(output.status.code(), Some(1), "{output:?}");
            let stdout = String::from_utf8_lossy(&output.stderr);
            assert!(
                stdout.contains("RINZU_NATIVE_CALLS=3,3,3,3,3,3,48,576,0,0,0"),
                "{stdout}"
            );
            assert!(
                stdout.contains("RINZU_FOREIGN=72,72,72,72,0,0,0,0"),
                "{stdout}"
            );
            assert!(
                stdout.contains(&format!("RINZU_FAILURE={name}"))
                    && stdout.contains("\"resources\":\"baseline\""),
                "{stdout}"
            );
            assert!(
                stdout.find("FOREIGN_NATIVE_RETURN").unwrap()
                    < stdout.find("FOREIGN_FINISH_AFTER_RETURN").unwrap(),
                "{stdout}"
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
        };
        let binary = app.join("build/foreign_app");
        check(&run(&binary));
        sign(&binary, &root);
        check(&run(&binary));
        check(&run(&sanitizer(&app)));
        success(
            &Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&app)
                .args([flag, "80"])
                .output()
                .unwrap(),
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn foreign_return_close_race_keeps_admitted_roots_and_rejects_later_entry() {
    for mode in [4, 5] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-cli-foreign-return-{mode}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        let app = root.join("app");
        std::fs::write(
            app.join("src/main.rinzu"),
            fixture::with_mode(
                &include_str!("fixtures/native_foreign_sdk/main.rinzu")
                    .replace("foreign.reset(0)", "foreign.reset(1)"),
                mode,
            ),
        )
        .unwrap();
        success(
            &Command::new(frozen_cli::path())
                .arg("build")
                .arg(&app)
                .output()
                .unwrap(),
        );
        for sanitized in [false, true] {
            let binary = return_race_binary(&app, sanitized);
            if !sanitized {
                sign(&binary, &root);
            }
            let output = Command::new(binary)
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stderr);
            if mode == 4 {
                success(&output);
                assert!(
                    stdout.contains("RINZU_NATIVE_CALLS=11,11,11,11,11,11,8,136,0,0,0"),
                    "{stdout}"
                );
                assert!(stdout.contains("RINZU_FOREIGN=8,8,8,8,0,0,0,0"), "{stdout}");
                assert!(
                    stdout.contains("RINZU_STARTED_CHILDREN=0,8")
                        && stdout.contains("\"resources\":\"baseline\""),
                    "{stdout}"
                );
                assert!(
                    stdout.find("FOREIGN_NATIVE_RETURN").unwrap()
                        < stdout.find("FOREIGN_FINISH_AFTER_RETURN").unwrap()
                );
            } else {
                assert_eq!(output.status.code(), Some(86), "{output:?}");
                assert!(!stdout.contains("FOREIGN_REJECTED_BODY"));
            }
            assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn foreign_signature_and_nested_progress_rejection_precede_fallback_or_body() {
    for (name, source, reason) in fixture::negative_sources(1) {
        let root = std::env::temp_dir().join(format!(
            "rinzu-cli-foreign-negative-{name}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        let app = root.join("app");
        std::fs::write(app.join("src/main.rinzu"), source).unwrap();
        success(
            &Command::new(frozen_cli::path())
                .arg("build")
                .arg(&app)
                .output()
                .unwrap(),
        );
        let binary = app.join("build/foreign_app");
        let check = |rejected: std::process::Output, engine| {
            assert_eq!(
                rejected.status.code(),
                Some(86),
                "{name}/{engine}: {rejected:?}"
            );
            assert!(!String::from_utf8_lossy(&rejected.stdout).contains("FOREIGN_REJECTED_BODY"));
            let diagnostic = String::from_utf8_lossy(&rejected.stderr);
            assert!(
                diagnostic.contains(&format!("foreign-dispatch-unavailable reason={reason} "))
                    && !diagnostic.contains("source=0:")
                    && diagnostic.contains("owner=0x")
                    && diagnostic.contains("rinzu foreign stack: symbol="),
                "{name}/{engine}: {diagnostic}"
            );
            assert!(
                !diagnostic.contains("panic")
                    && !diagnostic.contains("observable-wait-cycle")
                    && !diagnostic.contains("runtime error:")
                    && !diagnostic.contains("ERROR: AddressSanitizer"),
                "{diagnostic}"
            );
            if name == "bounded" {
                assert!(
                    diagnostic.contains("symbol=foreign_nested")
                        && diagnostic.contains("symbol=foreign_run"),
                    "{diagnostic}"
                );
            }
        };
        check(
            Command::new(frozen_cli::path())
                .arg("run")
                .arg(&app)
                .output()
                .unwrap(),
            "jit",
        );
        check(Command::new(&binary).output().unwrap(), "aot");
        sign(&binary, &root);
        check(Command::new(&binary).output().unwrap(), "signed");
        let sanitized = sanitizer(&app);
        check(
            Command::new(sanitized)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap(),
            "sanitized",
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
