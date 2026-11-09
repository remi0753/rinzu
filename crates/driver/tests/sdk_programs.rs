#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[allow(dead_code)]
#[path = "support/source_platform.rs"]
mod platform;
#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use scratch::Scratch;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn run_source(command: &mut Command, marker: &str) {
    let output = platform::bounded_tool(command);
    platform::success(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(marker),
        "{output:?}"
    );
}

fn stage_sdk(work: &Scratch, name: &str) -> PathBuf {
    platform::stage_fixture(work.path(), Path::new("sdk").join(name))
}

fn cli(command: &str, path: &Path, cwd: &Path) -> Command {
    let mut cli = Command::new(frozen_cli::path());
    cli.arg(command)
        .arg(path)
        .current_dir(cwd)
        .env("TMPDIR", cwd)
        .env("RINZU_CACHE_DIR", cwd.join("cache"));
    cli
}

#[test]
fn socket_packages_cover_wire_decoding_and_loopback_transfer() {
    let work = Scratch::new("rinzu-socket-programs");
    let tests = stage_sdk(&work, "socket").join("tests");
    for path in ["transfer-decode.rinzu", "transfer-receipt.rinzu"] {
        run_source(
            &mut cli("verify-aot", &tests.join(path), work.path()),
            "JIT/AOT equivalent:",
        );
    }
    run_source(
        &mut cli("run", &tests.join("app"), work.path()),
        "CLEANUP:0:0:0:0:",
    );
}

#[test]
fn process_environment_errors_and_owned_values_match_backends() {
    let work = Scratch::new("rinzu-process-program");
    let app = stage_sdk(&work, "process").join("tests/app");
    run_source(
        cli("verify-aot", &app, work.path())
            .env_remove("RINZU_PROCESS_TEST_ABSENT")
            .env("RINZU_PROCESS_TEST_VALUE", "日本 e\u{301} 😀")
            .env("RINZU_PROCESS_TEST_EMPTY", "")
            .env("RINZU_PROCESS_TEST_INTEGER", "255")
            .env("RINZU_PROCESS_TEST_TOO_LONG", "x".repeat(4097))
            .env(
                "RINZU_PROCESS_TEST_INVALID_UTF8",
                OsString::from_vec(vec![255]),
            ),
        "JIT/AOT equivalent:",
    );
}

#[test]
fn stream_and_log_programs_release_their_files() {
    let work = Scratch::new("rinzu-stream-log-programs");
    let stream = stage_sdk(&work, "stream").join("tests");
    let log = stage_sdk(&work, "log").join("tests");
    std::fs::write(work.path().join("input.bin"), [0xa5; 8192]).unwrap();
    run_source(
        &mut cli("run", &stream.join("parallel.rinzu"), work.path()),
        "stream-parallel-ok",
    );
    let cancelled =
        platform::bounded_tool(&mut cli("run", &stream.join("stress.rinzu"), work.path()));
    assert_eq!(cancelled.status.code(), Some(1), "{cancelled:?}");
    assert_eq!(cancelled.stdout, b"stream-stress-ok\n", "{cancelled:?}");
    assert!(
        String::from_utf8_lossy(&cancelled.stderr).contains("RINZU_FAILURE=cancelled"),
        "{cancelled:?}",
    );
    run_source(&mut cli("run", &log, work.path()), "log-ok");
}

#[test]
fn codec_and_sqlite_packages_execute_their_assertions() {
    let work = Scratch::new("rinzu-codec-sqlite-programs");
    let codec = stage_sdk(&work, "codec").join("tests/app");
    let sqlite = stage_sdk(&work, "sqlite").join("tests");
    run_source(
        &mut cli("verify-aot", &codec, work.path()),
        "JIT/AOT equivalent:",
    );
    run_source(
        &mut cli("run", &sqlite.join("rows"), work.path()),
        "sqlite-owned-rows-ok 256 65536",
    );
    let owned = sqlite.join("owned");
    std::fs::create_dir_all(owned.join("src")).unwrap();
    std::fs::copy(sqlite.join("rows/rinzu.toml"), owned.join("rinzu.toml")).unwrap();
    std::fs::copy(sqlite.join("owned.rinzu"), owned.join("src/main.rinzu")).unwrap();
    run_source(&mut cli("run", &owned, work.path()), "owned-ok");
}

#[test]
fn socket_sdk_client_and_server_transfer_across_processes() {
    run_python("test_socket_pair.py");
}

#[test]
fn http_sdk_checks_partial_responses_cancellation_and_tls_on_loopback() {
    run_python("test_sdk_http.py");
}

fn run_python(script: &str) {
    let work = Scratch::new("rinzu-sdk-python");
    let output = platform::bounded_tool(
        Command::new("python3")
            .arg(repository().join("tests/integration").join(script))
            .arg(frozen_cli::path())
            .env("TMPDIR", work.path()),
    );
    platform::success(&output);
}
