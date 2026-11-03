#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use source_edit::ReplaceChecked;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_supplant_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/native_supplant_sdk/preparation.rs"]
mod preparation;

fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}

fn baseline(output: &Output) {
    success(output);
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("RINZU_NATIVE_CALLS=24,24,24,24,24,24,2,0,0,0,0"),
        "{stdout}"
    );
    assert!(stdout.contains("RINZU_STARTED_CHILDREN=2,5"), "{stdout}");
    assert!(stdout.contains("RINZU_NATIVE_COMPLETIONS=4"), "{stdout}");
    assert!(stdout.contains("RINZU_NATIVE_ABANDONED=0"), "{stdout}");
    assert!(
        stdout.contains("RINZU_FAILURE=none") && stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .find(|line| line.starts_with("RINZU_NATIVE_ENTRIES="))
            .unwrap()
            .ends_with(",0,0"),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
        "{stderr}"
    );
}
fn sanitizer(app: &Path) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = app.join("build/supplants-sanitized");
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
            .arg(app.join("build/supplant_app.o"))
            .arg(app.join("build/supplant_app.runtime.c"))
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
        app.join("build/supplant_app.resources"),
        binary.with_file_name("supplants-sanitized.resources"),
    )
    .unwrap();
    binary
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
    let actual = String::from_utf8(actual.stdout).unwrap();
    assert!(
        actual.contains("disable-library-validation")
            && !actual.contains("allow-jit")
            && !actual.contains("allow-unsigned-executable-memory")
    );
}
fn exercise(label: &str, source: &str) {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-native-supplants-{label}-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        source.replace("loops.reset(0)", "loops.reset(1)"),
    )
    .unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("run")
        .arg(&app)
        .output()
        .unwrap();
    success(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("compiled JIT continuation runtime"));
    let build = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&app)
        .output()
        .unwrap();
    success(&build);
    let aot = Command::new(app.join("build/supplant_app"))
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    baseline(&aot);
    let binary = app.join("build/supplant_app");
    sign(&binary, &root);
    baseline(
        &Command::new(&binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap(),
    );
    baseline(
        &Command::new(sanitizer(&app))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap(),
    );
    let verify = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&app)
        .output()
        .unwrap();
    success(&verify);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn supplants_enter_on_process_main_after_the_synchronous_prefix() {
    exercise(
        "direct",
        include_str!("fixtures/native_supplant_sdk/main.rinzu"),
    );
}
#[test]
fn supplant_references_keep_their_effect_and_select_a_fresh_physical_plan() {
    let source=include_str!("fixtures/native_supplant_sdk/main.rinzu").replace_checked(
        "let result = await loops.run(callback, round);",
        "var selected=loops.run; if round==2 {selected=loops.runAlias;} let result=await selected(callback,round);"
    );
    exercise("references", &source);
}

fn gate_output(output: &Output, failure: &str, abandoned: usize, callbacks: usize) {
    let stdout = String::from_utf8_lossy(&output.stderr);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(i32::from(failure != "none")),
        "{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("RINZU_FAILURE={failure}"))
            && stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("RINZU_NATIVE_ABANDONED={abandoned}")),
        "{stdout}"
    );
    let phases: Vec<usize> = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_NATIVE_CALLS="))
        .unwrap()
        .split(',')
        .map(|part| part.parse().unwrap())
        .collect();
    assert_eq!(phases[0], phases[1] + abandoned, "{stdout}");
    assert!(
        phases[1..6].iter().all(|phase| *phase == phases[1]),
        "{stdout}"
    );
    assert_eq!(phases[6], callbacks, "{stdout}");
    assert_eq!(&phases[8..], &[0, 0, 0], "{stdout}");
    assert_eq!(
        stdout
            .lines()
            .filter(|line| *line == "SUPPLANT_CALLBACK_FINALLY")
            .count(),
        callbacks,
        "{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|line| *line == "SUPPLANT_NATIVE_RETURN")
            .count(),
        callbacks,
        "{stdout}"
    );
    assert_eq!(
        stdout
            .lines()
            .filter(|line| *line == "SUPPLANT_OUTER_FINALLY")
            .count(),
        1,
        "{stdout}"
    );
    if callbacks == 1 {
        let inner = stdout.find("SUPPLANT_CALLBACK_FINALLY").unwrap();
        let physical = stdout.find("SUPPLANT_NATIVE_RETURN").unwrap();
        let outer = stdout.find("SUPPLANT_OUTER_FINALLY").unwrap();
        assert!(inner < physical && physical < outer, "{stdout}");
    }
    assert!(
        stdout
            .lines()
            .find(|line| line.starts_with("RINZU_NATIVE_ENTRIES="))
            .unwrap()
            .ends_with(",0,0"),
        "{stdout}"
    );
    assert!(
        !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
        "{stderr}"
    );
}

#[test]
fn supplant_cancellation_and_result_error_gates_settle_before_restart_or_outer_finally() {
    for (name, source, cancel, failure, abandoned, callbacks) in [
        (
            "queued",
            include_str!("fixtures/native_supplant_sdk/queued-cancel.rinzu"),
            None,
            "cancelled",
            1,
            0,
        ),
        (
            "running",
            include_str!("fixtures/native_supplant_sdk/cancel.rinzu"),
            Some("80"),
            "cancelled",
            0,
            1,
        ),
        (
            "restart",
            include_str!("fixtures/native_supplant_sdk/result-restart.rinzu"),
            None,
            "none",
            0,
            2,
        ),
    ] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-supplant-cli-gate-{name}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        let app = root.join("app");
        std::fs::write(
            app.join("src/main.rinzu"),
            source.replace("loops.checkReset(0)", "loops.checkReset(1)"),
        )
        .unwrap();
        success(
            &Command::new(frozen_cli::path())
                .arg("build")
                .arg(&app)
                .output()
                .unwrap(),
        );
        let run = |binary: &Path| {
            let mut command = Command::new(binary);
            command
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0");
            if let Some(limit) = cancel {
                command.env("RINZU_CANCEL_AFTER_POLLS", limit);
            }
            command.output().unwrap()
        };
        let binary = app.join("build/supplant_app");
        gate_output(&run(&binary), failure, abandoned, callbacks);
        sign(&binary, &root);
        gate_output(&run(&binary), failure, abandoned, callbacks);
        gate_output(&run(&sanitizer(&app)), failure, abandoned, callbacks);
        let mut verify = Command::new(frozen_cli::path());
        verify.arg("verify-aot").arg(&app);
        if let Some(limit) = cancel {
            verify.args(["--cancel-after-polls", limit]);
        }
        success(&verify.output().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn prepare_fault_binary(app: &Path, sanitized: bool) -> PathBuf {
    let source = std::fs::read_to_string(app.join("build/supplant_app.runtime.c")).unwrap();
    let source = preparation::instrument(source, None);
    let path = app.join(if sanitized {
        "build/supplant-prepare-sanitized.c"
    } else {
        "build/supplant-prepare.c"
    });
    let binary = path.with_extension("");
    std::fs::write(&path, source).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("clang");
    command
        .args([
            "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
        ])
        .arg(app.join("build/supplant_app.o"))
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
    binary
}

#[test]
fn worker_native_blocker_preserves_physical_return_on_origin_cancel_and_shutdown() {
    let root =
        std::env::temp_dir().join(format!("rinzu-worker-blocker-cli-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("fixtures/native_supplant_sdk/worker-blocker.rinzu")
            .replace("loops.checkReset(0)", "loops.checkReset(1)"),
    )
    .unwrap();
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    let check = |output: &Output, failure| {
        gate_output(output, failure, 0, 0);
        let stdout = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("RINZU_NATIVE_CALLS=5,5,5,5,5,5,0,0,0,0,0"),
            "{stdout}"
        );
        assert!(stdout.contains("RINZU_STARTED_CHILDREN=1,0"), "{stdout}");
        let worker = stdout
            .lines()
            .find_map(|line| line.strip_prefix("RINZU_WORKER_HEAPS="))
            .unwrap();
        let parts = worker.split(':').collect::<Vec<_>>();
        assert_eq!(&parts[1..3], &["0", "0"], "{worker}");
        assert!(
            parts[3].parse::<usize>().unwrap() > 50 && parts[5].parse::<usize>().unwrap() >= 2,
            "{worker}"
        );
        let warnings = String::from_utf8_lossy(&output.stderr);
        let blocker = warnings
            .lines()
            .find(|line| line.contains("shutdown-blocker symbol=supplant_worker_block "))
            .unwrap_or_else(|| panic!("{warnings}"));
        assert!(
            !blocker.contains("source=0:")
                && blocker.contains("owner=0x")
                && blocker.contains("stack=supplant_worker_block"),
            "{blocker}"
        );
        assert_eq!(
            warnings.matches("shutdown-blocker ").count(),
            1,
            "{warnings}"
        );
        let native_return = warnings.find("WORKER_BLOCKER_NATIVE_RETURN").unwrap();
        let worker_finally = warnings.find("WORKER_BLOCKER_WORKER_FINALLY").unwrap();
        assert!(
            warnings
                .find("WORKER_BLOCKER_ORIGIN_OBSERVED_ENTRY")
                .unwrap()
                < native_return
                && warnings.find("shutdown-blocker ").unwrap() < native_return
                && native_return < worker_finally,
            "{warnings}"
        );
    };
    let run = |binary: &Path, variable| {
        Command::new(binary)
            .env(variable, "80")
            .env("RINZU_GC_STRESS", "1")
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap()
    };
    let binary = app.join("build/supplant_app");
    for signed in [false, true] {
        if signed {
            sign(&binary, &root);
        }
        for (variable, failure) in [
            ("RINZU_CANCEL_AFTER_POLLS", "cancelled"),
            ("RINZU_SHUTDOWN_AFTER_POLLS", "shutdown"),
        ] {
            check(&run(&binary, variable), failure);
        }
    }
    let sanitized = sanitizer(&app);
    for (variable, failure, flag) in [
        (
            "RINZU_CANCEL_AFTER_POLLS",
            "cancelled",
            "--cancel-after-polls",
        ),
        (
            "RINZU_SHUTDOWN_AFTER_POLLS",
            "shutdown",
            "--shutdown-after-polls",
        ),
    ] {
        check(&run(&sanitized, variable), failure);
        let verified = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .args([flag, "80"])
            .output()
            .unwrap();
        success(&verified);
        assert!(
            String::from_utf8_lossy(&verified.stdout).contains(&format!("outcome={failure}")),
            "{verified:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supplant_shutdown_preserves_callback_returns_and_joins_origin_children() {
    for (name, source, limit, abandoned, callbacks) in [
        (
            "queued",
            include_str!("fixtures/native_supplant_sdk/queued-cancel.rinzu"),
            "40",
            1,
            0,
        ),
        (
            "running",
            include_str!("fixtures/native_supplant_sdk/cancel.rinzu"),
            "80",
            0,
            1,
        ),
        (
            "child",
            include_str!("fixtures/native_supplant_sdk/shutdown-child.rinzu"),
            "80",
            0,
            1,
        ),
        (
            "blocker",
            include_str!("fixtures/native_supplant_sdk/blocker.rinzu"),
            "80",
            0,
            1,
        ),
    ] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-supplant-cli-shutdown-{name}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        let app = root.join("app");
        std::fs::write(
            app.join("src/main.rinzu"),
            source.replace("loops.checkReset(0)", "loops.checkReset(1)"),
        )
        .unwrap();
        success(
            &Command::new(frozen_cli::path())
                .arg("build")
                .arg(&app)
                .output()
                .unwrap(),
        );
        let run = |binary: &Path| {
            Command::new(binary)
                .env("RINZU_SHUTDOWN_AFTER_POLLS", limit)
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap()
        };
        let check = |output: &Output| {
            gate_output(output, "shutdown", abandoned, callbacks);
            if name == "blocker" {
                let warnings = String::from_utf8_lossy(&output.stderr);
                let warning = warnings
                    .lines()
                    .find(|line| line.contains("shutdown-blocker symbol=supplant_check_run "))
                    .unwrap_or_else(|| panic!("{warnings}"));
                assert!(
                    !warning.contains("source=0:") && warning.contains("stack=supplant_check_run"),
                    "{warning}"
                );
                assert_eq!(
                    warnings.matches("shutdown-blocker ").count(),
                    1,
                    "{warnings}"
                );
                assert!(
                    warnings.contains("SUPPLANT_BLOCKED_AFTER_CALLBACK"),
                    "{warnings}"
                );
            }
            if name == "child" {
                let stdout = String::from_utf8_lossy(&output.stderr);
                assert!(stdout.contains("SUPPLANT_CHILD_FINALLY"), "{stdout}");
                assert!(
                    stdout.find("SUPPLANT_NATIVE_RETURN").unwrap()
                        < stdout.find("SUPPLANT_CHILD_FINALLY").unwrap(),
                    "{stdout}"
                );
            }
        };
        let binary = app.join("build/supplant_app");
        check(&run(&binary));
        sign(&binary, &root);
        check(&run(&binary));
        check(&run(&sanitizer(&app)));
        let verified = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .args(["--shutdown-after-polls", limit])
            .output()
            .unwrap();
        success(&verified);
        assert!(
            String::from_utf8_lossy(&verified.stdout).contains("outcome=shutdown"),
            "{verified:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn supplant_actual_preparation_allocation_failures_never_enter_native_or_cleanup() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-supplant-prepare-fault-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("fixtures/native_supplant_sdk/result-restart.rinzu")
            .replace("loops.checkReset(0)", "loops.checkReset(1)"),
    )
    .unwrap();
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    let mut expected = None;
    for sanitized in [false, true] {
        let binary = prepare_fault_binary(&app, sanitized);
        let run = |budget: usize| {
            Command::new(&binary)
                .env("RINZU_PREPARE_BUDGET", budget.to_string())
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap()
        };
        let complete = run(10000);
        success(&complete);
        let stdout = String::from_utf8_lossy(&complete.stderr);
        let count: usize = stdout
            .lines()
            .find_map(|line| line.strip_prefix("SUPPLANT_PREPARE_ALLOCATIONS="))
            .unwrap()
            .parse()
            .unwrap();
        assert!((6..10000).contains(&count), "{stdout}");
        assert_eq!(*expected.get_or_insert(count), count);
        for budget in 0..count {
            let output = run(budget);
            let stdout = String::from_utf8_lossy(&output.stderr);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.code() == Some(71),
                "sanitized={sanitized} budget={budget}: {stdout}\n{stderr}"
            );
            assert!(
                !stderr.contains("SUPPLANT_NATIVE_ENTER")
                    && !stdout.contains("SUPPLANT_CALLBACK_FINALLY")
                    && !stdout.contains("SUPPLANT_OUTER_FINALLY")
                    && !stdout.contains("RINZU_RESULT="),
                "{stdout}\n{stderr}"
            );
            assert!(
                !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
        }
    }
    eprintln!(
        "verified {} actual supplant preparation allocations",
        expected.unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}
