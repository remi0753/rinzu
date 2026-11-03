#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn native_http_codec_preserves_request_line_fields_status_and_gc() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for stress in [false, true] {
        let run = std::process::Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(root.join("tests/fixtures/httpcodec.rinzu"))
            .env("RINZU_GC_STRESS", if stress { "1" } else { "0" })
            .output()
            .unwrap();
        assert!(run.status.success(), "{run:?}");
        assert!(String::from_utf8_lossy(&run.stdout).contains("outcome=success"));
    }
}

#[cfg(unix)]
#[test]
fn native_request_line_kernel_matches_mutations_under_sanitizers() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::env::temp_dir().join(format!("rinzu-httpcodec-kernel-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let output = std::process::Command::new("python3")
        .arg(root.join("tests/fixtures/http/test_httpcodec.py"))
        .arg("--workdir")
        .arg(&work)
        .arg("--output")
        .arg(work.join("result.json"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(work);
    assert!(output.status.success(), "{output:?}");
}
