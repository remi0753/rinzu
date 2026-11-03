#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn url_spans_match_bytewise_values_and_errors_under_gc() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::env::temp_dir().join(format!("rinzu-url-spans-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let run = std::process::Command::new("python3")
        .arg(root.join("tests/fixtures/http/test_url_spans.py"))
        .arg("--rinzu")
        .arg(frozen_cli::path())
        .arg("--before")
        .arg(root.join("tests/fixtures/http/trials/url-bytewise.rinzu"))
        .arg("--workdir")
        .arg(&work)
        .arg("--output")
        .arg(work.join("result.json"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(work);
    assert!(run.status.success(), "{run:?}");
}
