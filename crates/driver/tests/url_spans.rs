#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn url_spans_match_bytewise_values_and_errors_under_gc() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = scratch::Scratch::new("rinzu-url-spans");
    let run = std::process::Command::new("python3")
        .arg(root.join("tests/integration/test_url_spans.py"))
        .arg("--rinzu")
        .arg(frozen_cli::path())
        .arg("--before")
        .arg(root.join("tests/fixtures/http/trials/url-bytewise.rinzu"))
        .arg("--workdir")
        .arg(work.path())
        .arg("--output")
        .arg(work.path().join("result.json"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("TMPDIR", work.path())
        .env("RINZU_CACHE_DIR", work.path().join("cache"))
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
}
