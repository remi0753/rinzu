#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn byte_set_scans_and_ascii_case_preserve_binary_bounds_and_evaluation_order() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../codegen/tests/fixtures/byte_scans.rinzu");
    for stress in [false, true] {
        let output = std::process::Command::new(frozen_cli::path())
            .args(["verify-aot"])
            .arg(&source)
            .env("RINZU_GC_STRESS", if stress { "1" } else { "0" })
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
        let worker = source.with_file_name("byte_predicates_worker.rinzu");
        let output = std::process::Command::new(frozen_cli::path())
            .args(["verify-aot"])
            .arg(worker)
            .env("RINZU_GC_STRESS", if stress { "1" } else { "0" })
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
}
