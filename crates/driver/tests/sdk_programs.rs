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
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
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

fn no_replace_temporaries(directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(
            !name.to_string_lossy().starts_with(".rinzu-replace-"),
            "{name:?}"
        );
    }
}

fn check_whole_run(command: &mut Command, output: &str, cancelled: bool, close_errors: bool) {
    let result = platform::bounded_tool(command);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_eq!(
        result.status.code(),
        Some(i32::from(cancelled)),
        "{result:?}"
    );
    assert_eq!(result.stdout, output.as_bytes(), "{result:?}");
    if cancelled {
        assert!(stderr.contains("RINZU_FAILURE=cancelled"), "{stderr}");
    }
    if close_errors {
        let diagnostics = stderr
            .lines()
            .find_map(|line| line.strip_prefix("RINZU_DIAGNOSTICS="))
            .unwrap_or_else(|| panic!("missing failure diagnostics: {stderr}"));
        let kinds: Vec<_> = diagnostics
            .split(',')
            .map(|diagnostic| diagnostic.split('@').next().unwrap())
            .collect();
        assert_eq!(kinds, ["2", "15", "15", "15"], "{stderr}");
    }
}

#[test]
fn io_whole_file_public_contracts_match_on_jit_and_aot() {
    let work = Scratch::new("rinzu-io-whole-program");
    let source = stage_sdk(&work, "io").join("tests/whole_file.rinzu");
    let binary = work.path().join("whole-file");
    platform::success(&platform::bounded_tool(
        cli("build", &source, work.path())
            .arg("--output")
            .arg(&binary),
    ));
    for backend in ["jit", "aot"] {
        let directory = work.path().join(backend);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("empty.bin"), []).unwrap();
        fs::write(directory.join("exact.bin"), vec![42; 65536]).unwrap();
        fs::write(directory.join("over.bin"), vec![42; 65537]).unwrap();
        fs::write(directory.join("invalid.bin"), [0xff]).unwrap();
        fs::write(directory.join("before-start.txt"), b"old").unwrap();
        symlink("exact.bin", directory.join("link.bin")).unwrap();
        let mut boundary = b"\xef\xbb\xbf\x00".to_vec();
        boundary.resize(65535, b'a');
        boundary.extend_from_slice("🙂\n".as_bytes());
        fs::write(directory.join("boundary.txt"), boundary).unwrap();
        let mut command = if backend == "jit" {
            cli("run", &source, &directory)
        } else {
            let mut command = Command::new(&binary);
            command.current_dir(&directory).env("TMPDIR", &directory);
            command
        };
        check_whole_run(
            &mut command,
            "whole-file-ok\nwhole-file-cancel-before-start\n",
            true,
            false,
        );
        assert_eq!(
            fs::read(directory.join("before-start.txt")).unwrap(),
            b"old"
        );
        assert!(fs::read(directory.join("output.bin")).unwrap().is_empty());
        no_replace_temporaries(&directory);
    }
}

fn stage_whole_probe(work: &Scratch) -> PathBuf {
    let sdk = stage_sdk(work, "io");
    let manifest = sdk.join("rinzu.toml");
    let contents = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        contents
            .replace("rinzu.io", "whole_io_probe")
            .replace("native/src/whole_file.c", "tests/whole_file.c"),
    )
    .unwrap();
    let native = sdk.join("tests/whole_file.c");
    let contents = fs::read_to_string(&native).unwrap();
    fs::write(
        &native,
        format!("#define RINZU_WHOLE_FILE_PROBE\n{contents}"),
    )
    .unwrap();
    let header = sdk.join("native/include/rinzu_io.h");
    let contents = fs::read_to_string(&header).unwrap();
    fs::write(
        &header,
        contents
            + r#"
void whole_test_fault(int64_t mode);
void whole_test_gate(int64_t stage);
bool whole_test_entered(void);
bool whole_test_cancelled(void);
int64_t whole_test_outcome(void);
bool whole_test_remove_name(const char *name);
"#,
    )
    .unwrap();
    let source = sdk.join("src/io.rinzu");
    let contents = fs::read_to_string(&source).unwrap();
    fs::write(
        &source,
        contents + "\n" + &fs::read_to_string(sdk.join("tests/whole_file_probe.rinzu")).unwrap(),
    )
    .unwrap();
    let app = work.path().join("app");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("rinzu.toml"),
        r#"
[package]
name = "whole_file_faults"
language = "0.5"
[dependencies]
whole_io_probe = { path = "../sdk/io" }
"#,
    )
    .unwrap();
    let contents = fs::read_to_string(sdk.join("tests/whole_file_faults.rinzu")).unwrap();
    fs::write(
        app.join("src/main.rinzu"),
        contents
            .replace("import rinzu.io as io;", "import whole_io_probe as io;")
            .replace("import \"./whole_file_probe.rinzu\" as probe;", "")
            .replace("probe.", "io."),
    )
    .unwrap();
    app
}

#[test]
fn io_whole_file_faults_and_cancel_install_races_match_on_jit_and_aot() {
    let work = Scratch::new("rinzu-io-whole-faults");
    let app = stage_whole_probe(&work);
    let binary = work.path().join("whole-file-faults");
    platform::success(&platform::bounded_tool(
        cli("build", &app, work.path()).arg("--output").arg(&binary),
    ));
    for backend in ["jit", "aot"] {
        for (mode, marker, contents) in [
            ("faults", "whole-file-faults-ok\n", b"new"),
            ("before", "whole-file-control-ok:5\n", b"old"),
            ("during", "whole-file-control-ok:6\n", b"new"),
            ("after", "whole-file-control-ok:7\n", b"new"),
            ("timeout", "whole-file-control-ok:5\n", b"old"),
        ] {
            let directory = work.path().join(format!("{backend}-{mode}"));
            fs::create_dir(&directory).unwrap();
            let destination = directory.join("destination");
            fs::write(&destination, b"old").unwrap();
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o640)).unwrap();
            fs::write(directory.join("invalid.bin"), [0xff]).unwrap();
            platform::success(&platform::bounded_tool(
                Command::new("xattr")
                    .args(["-w", "org.rinzu.test", "keep"])
                    .arg(&destination),
            ));
            let mut command = if backend == "jit" {
                let mut command = cli("run", &app, &directory);
                command.env("RINZU_CACHE_DIR", work.path().join("cache"));
                command
            } else {
                let mut command = Command::new(&binary);
                command.current_dir(&directory).env("TMPDIR", &directory);
                command
            };
            if mode != "faults" {
                if backend == "jit" {
                    command.arg("--");
                }
                command.arg(mode);
            }
            check_whole_run(&mut command, marker, mode != "faults", mode == "after");
            assert_eq!(&fs::read(&destination).unwrap(), contents);
            assert_eq!(
                fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
                0o640
            );
            let attribute = platform::bounded_tool(
                Command::new("xattr")
                    .args(["-p", "org.rinzu.test"])
                    .arg(&destination),
            );
            platform::success(&attribute);
            assert_eq!(attribute.stdout, b"keep\n");
            no_replace_temporaries(&directory);
        }
    }
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
