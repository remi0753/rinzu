use std::path::PathBuf;
use std::process::Command;

fn workspace_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[test]
fn valid_trace_and_schema_commands_pass() {
    let binary = env!("CARGO_BIN_EXE_rinzu-conformance");
    let trace = Command::new(binary)
        .args(["trace", "validate"])
        .arg(workspace_path("tests/conformance/trace/valid/basic.json"))
        .output()
        .unwrap();
    assert!(
        trace.status.success(),
        "{}",
        String::from_utf8_lossy(&trace.stderr)
    );

    let schema = Command::new(binary).arg("trace-schema").output().unwrap();
    assert!(
        schema.status.success(),
        "{}",
        String::from_utf8_lossy(&schema.stderr)
    );
}

#[test]
fn invalid_trace_command_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_rinzu-conformance"))
        .args(["trace", "validate"])
        .arg(workspace_path(
            "tests/conformance/trace/invalid/noncontiguous.json",
        ))
        .output()
        .unwrap();
    assert!(!output.status.success());
}
