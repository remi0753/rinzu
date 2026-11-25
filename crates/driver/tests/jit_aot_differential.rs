#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[allow(dead_code)]
#[path = "../../../tests/support/process.rs"]
mod process;
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use rinzu_compiler::compatibility::LANGUAGE;
use std::path::Path;
use std::process::Command;

fn verify(input: &str, controls: &[&str], expected: &str) {
    let scratch = scratch::Scratch::new("rinzu-jit-aot-differential");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join(input);
    let mut command = Command::new(frozen_cli::path());
    command
        .current_dir(scratch.path())
        .env("RINZU_CACHE_DIR", scratch.path().join("cache"))
        .arg("verify-aot")
        .arg(&source);
    if source.is_dir() {
        let manifest = rinzu_compiler::load_manifest(&source.join("rinzu.toml")).unwrap();
        assert_eq!(manifest.language, LANGUAGE, "{input}");
    } else {
        command.args(["--language", LANGUAGE]);
    }
    let output = process::bounded_tool(command.args(controls));
    assert!(output.status.success(), "{input}: {output:?}");
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    for evidence in [
        expected,
        "resume-domain=main",
        "resources=baseline",
        "shutdown=complete",
    ] {
        assert!(stdout.contains(evidence), "{input}: {stdout}");
    }
}

#[test]
fn synchronous_package_returns_successfully() {
    verify("../compiler/tests/fixtures/package", &[], "outcome=success");
}

#[test]
fn async_future_observation_returns_successfully() {
    verify(
        "../codegen/tests/fixtures/future-timeout-observation.rinzu",
        &[],
        "outcome=success",
    );
}

#[test]
fn arithmetic_trap_reports_failure() {
    verify("tests/fixtures/aot_failure", &[], "outcome=failure");
}

#[test]
fn cpu_poll_reports_cancellation() {
    verify(
        "tests/fixtures/aot_cancel",
        &["--cancel-after-polls", "6"],
        "outcome=cancelled",
    );
}

#[test]
fn allocation_limit_reports_overload() {
    verify(
        "tests/fixtures/aot_overload",
        &["--max-allocations", "1"],
        "outcome=overloaded",
    );
}

#[test]
fn cpu_poll_reports_shutdown() {
    verify(
        "tests/fixtures/aot_cancel",
        &["--shutdown-after-polls", "6"],
        "outcome=shutdown",
    );
}
