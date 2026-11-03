#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn empty_callable_containers_do_not_require_an_unused_concrete_target() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../codegen/tests/fixtures/uninhabited_callables.rinzu");
    let output = std::process::Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(source)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
}
