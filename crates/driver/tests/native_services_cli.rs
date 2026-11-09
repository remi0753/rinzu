#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/native_services_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
mod platform;
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;
use std::process::{Command, Output};

fn evidence(output: &Output, failure: &str, timers: usize) {
    assert_eq!(
        output.status.code(),
        Some(i32::from(failure != "none")),
        "{output:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains(&format!("RINZU_FAILURE={failure}"))
            && stdout.contains("\"resources\":\"baseline\""),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("RINZU_STARTED_TIMERS={timers}")),
        "{stdout}"
    );
    assert!(
        stdout.contains("RINZU_TRANSPORT=0,0,0,0,0,0"),
        "timer uses no CPU/transport admission: {stdout}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("runtime error:"),
        "{output:?}"
    );
}

#[test]
fn closed_timer_file_and_gc_run_with_only_source_service_declarations_in_aot_signed_and_sanitizers()
{
    let root = std::env::temp_dir().join(format!("rinzu-closed-timer-cli-{}", std::process::id()));
    fixture::create(&root, false);
    let app = root.join("app");
    let cli = |action| platform::bounded(Command::new(frozen_cli::path()).arg(action).arg(&app));
    platform::success(&cli("run"));
    platform::success(&cli("build"));
    let binary = app.join("build/services_app");
    let check = |output: Output| {
        evidence(&output, "none", 2);
        let stdout = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("RINZU_STARTED_CHILDREN=0,1")
                && stdout.contains("\"operation\":\"timer\""),
            "{stdout}"
        );
        assert!(stdout.contains("RINZU_CAUGHT_ERRORS=5@"), "{stdout}");
    };
    check(platform::bounded(
        Command::new(&binary).env("RINZU_TRANSPORT_EVIDENCE", "1"),
    ));
    let sanitized = platform::sanitize(&binary);
    check(platform::bounded(
        Command::new(&sanitized)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    check(platform::bounded(
        Command::new(&binary).env("RINZU_TRANSPORT_EVIDENCE", "1"),
    ));
    platform::success(&cli("verify-aot"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn timer_cancellation_and_shutdown_collect_the_real_deadline_before_scope_disposal() {
    let scratch = scratch::Scratch::new("rinzu-closed-timer-cancel-cli");
    let root = scratch.path();
    fixture::create(root, true);
    let app = root.join("app");
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/services_app");
    let sanitized = platform::sanitize(&binary);
    platform::sign(&binary, root);
    let polls = fixture::CONTROL_POLLS.to_string();
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
        for executable in [&binary, &sanitized] {
            let started = std::time::Instant::now();
            let output = platform::bounded(
                Command::new(executable)
                    .env(variable, &polls)
                    .env("ASAN_OPTIONS", "detect_leaks=0")
                    .env("UBSAN_OPTIONS", "halt_on_error=1"),
            );
            evidence(&output, failure, 1);
            assert!(
                started.elapsed() >= fixture::TIMER_DURATION,
                "cancellation cannot synthesize physical completion"
            );
        }
        platform::success(&platform::bounded_tool(
            Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&app)
                .args([flag, &polls]),
        ));
    }
}
