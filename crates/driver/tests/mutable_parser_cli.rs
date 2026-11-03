#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn mutable_parser_in_an_async_caller_matches_jit_and_aot() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/http/test-mutable-parser.rinzu");
    let output = std::process::Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(source)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
}
