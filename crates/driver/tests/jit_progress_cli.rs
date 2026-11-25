#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/scratch.rs"]
#[allow(dead_code)]
mod scratch;

use scratch::Scratch;
use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;
use std::process::{Command, Output, Stdio};

const PREFIX: &str = "[rinzu:jit] ";
const CI_VARIABLES: &[&str] = &[
    "CI",
    "CONTINUOUS_INTEGRATION",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "TF_BUILD",
    "JENKINS_URL",
    "TEAMCITY_VERSION",
    "BUILDKITE",
    "CIRCLECI",
    "TRAVIS",
];

struct Fixture(Scratch);

impl Fixture {
    fn new(source: &str) -> Self {
        let scratch = Scratch::new("rinzu-jit-progress");
        std::fs::write(scratch.path().join("main.rinzu"), source).unwrap();
        Self(scratch)
    }

    fn command(&self) -> Command {
        self.command_for("main.rinzu")
    }

    fn command_for(&self, input: &str) -> Command {
        let mut command = Command::new(frozen_cli::path());
        command
            .current_dir(self.0.path())
            .args(["run", input])
            .env("RINZU_CACHE_DIR", self.0.path().join("cache"))
            .env_remove("RINZU_TOOLCHAIN_ROOT")
            .env_remove("RINZU_COMPILE_TIMINGS")
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .stdin(Stdio::null());
        for name in CI_VARIABLES {
            command.env_remove(name);
        }
        command
    }

    fn dependency_project() -> Self {
        let fixture = Self::new("");
        for name in ["foundation", "middle", "app"] {
            std::fs::create_dir_all(fixture.0.path().join(name).join("src")).unwrap();
        }
        for (name, dependencies) in [
            ("foundation", ""),
            (
                "middle",
                "[dependencies]\nfoundation={path=\"../foundation\"}\n",
            ),
        ] {
            std::fs::write(fixture.0.path().join(name).join("rinzu.toml"), format!(
                "[package]\nname=\"{name}\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/lib.rinzu\"\n{dependencies}"
            )).unwrap();
        }
        std::fs::write(
            fixture.0.path().join("foundation/src/lib.rinzu"),
            "fn answer()->Int{return 40;} export answer;",
        )
        .unwrap();
        std::fs::write(
            fixture.0.path().join("middle/src/lib.rinzu"),
            "import foundation;fn answer()->Int{return foundation.answer()+2;}export answer;",
        )
        .unwrap();
        std::fs::write(fixture.0.path().join("app/rinzu.toml"),
            "[package]\nname=\"progress_app\"\nlanguage=\"0.5\"\n[dependencies]\nmiddle={path=\"../middle\"}\n").unwrap();
        std::fs::write(fixture.0.path().join("app/src/main.rinzu"),
            "import middle;import rinzu.stdio;fn main(){try{rinzu.stdio.writeLine(middle.answer());}catch errors{panic(\"write failed\");}}").unwrap();
        fixture
    }
}

fn synchronous_source() -> &'static str {
    r#"
import rinzu.stdio;
fn main() {
    try { rinzu.stdio.writeLine("APP_LOG"); }
    catch errors { panic("write failed"); }
}
"#
}

fn terminal_output(mut command: Command, stdout_tty: bool, stderr_tty: bool) -> (Output, String) {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty initializes two distinct owned descriptors. Optional
    // terminal settings, window size and the name buffer are all omitted.
    let status = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(status, 0);
    // SAFETY: openpty succeeded and ownership of each descriptor is transferred
    // exactly once into its corresponding File.
    let (mut master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    if stdout_tty {
        command.stdout(slave.try_clone().unwrap());
    }
    if stderr_tty {
        command.stderr(slave.try_clone().unwrap());
    }
    // Drain while the child is running: macOS can discard unread terminal
    // bytes when the last slave closes, and a full terminal can block a child.
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                // Linux reports the closed slave as EIO, macOS as EOF.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) => panic!("read terminal: {error}"),
            }
        }
        String::from_utf8(bytes).unwrap()
    });
    let output = command.output().unwrap();
    drop(command);
    drop(slave);
    (output, reader.join().unwrap())
}

fn progress_rows(stderr: &str) -> Vec<(f64, &str)> {
    let mut rows = Vec::new();
    for line in stderr.lines().filter_map(|line| line.strip_prefix(PREFIX)) {
        let (elapsed, message) = line.strip_prefix('+').unwrap().split_once("s ").unwrap();
        let elapsed = elapsed.parse::<f64>().unwrap();
        assert!(elapsed.is_finite() && elapsed >= 0.0);
        assert!(
            message.is_ascii(),
            "progress phase must be English: {message}"
        );
        rows.push((elapsed, message));
    }
    assert!(rows.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    rows
}

#[test]
fn explicit_progress_preserves_stdout_and_covers_sync_async_and_diagnostic_runs() {
    for asynchronous in [false, true] {
        let source = if asynchronous {
            r#"
import rinzu.stdio;
import rinzu.timer;
async fn main() {
    try {
        await rinzu.timer.sleep(1);
        rinzu.stdio.writeLine("APP_LOG");
    }
    catch errors { panic("application failed"); }
}
"#
        } else {
            synchronous_source()
        };
        let fixture = Fixture::new(source);
        for diagnostic in [false, true] {
            let mut command = fixture.command();
            command.arg("--progress=always");
            if diagnostic {
                command.args(["--diagnostic-report", "diagnostic.json"]);
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(output.stdout, b"APP_LOG\n");
            let stderr = String::from_utf8(output.stderr).unwrap();
            let rows = progress_rows(&stderr);
            assert!(rows.first().unwrap().1.starts_with("Preparing:"));
            assert!(
                rows.iter()
                    .any(|(_, text)| *text == "Checking frontend cache")
            );
            for phase in [
                "Parsing application sources",
                "Resolving dependencies",
                "Checking application types",
                "Preparing and verifying IR",
                "Preparing frontend cache",
            ] {
                assert_eq!(
                    rows.iter().any(|(_, text)| *text == phase),
                    !diagnostic,
                    "{stderr}"
                );
            }
            for phase in ["Validating cached inputs", "Verifying cached IR"] {
                assert_eq!(
                    rows.iter().any(|(_, text)| *text == phase),
                    diagnostic,
                    "{stderr}"
                );
            }
            assert!(!stderr.contains("Checking source and dependencies"));
            assert!(!stderr.contains("Waiting for native dependencies"));
            assert!(rows.iter().any(|(_, text)| *text == "Compiling JIT code"));
            assert_eq!(
                rows.iter()
                    .filter(|(_, text)| text.starts_with("Starting Main (startup="))
                    .count(),
                1
            );
            assert!(
                rows.last()
                    .unwrap()
                    .1
                    .starts_with("Finished: exit=0, execution=")
            );
        }
    }
}

#[test]
fn auto_and_explicit_modes_respect_terminals_pipes_and_ci() {
    let fixture = Fixture::new(synchronous_source());
    let piped = fixture.command().output().unwrap();
    assert!(piped.status.success(), "{piped:?}");
    assert_eq!(piped.stdout, b"APP_LOG\n");
    assert!(!String::from_utf8_lossy(&piped.stderr).contains(PREFIX));

    for (stdout_tty, stderr_tty, ci, mode, expected) in [
        (true, true, false, None, true),
        (true, true, false, Some("auto"), true),
        (false, true, false, None, false),
        (true, false, false, None, false),
        (true, true, true, None, false),
        (true, true, true, Some("always"), true),
        (true, true, false, Some("never"), false),
    ] {
        let mut command = fixture.command();
        if ci {
            command.env("CI", "true");
        }
        if let Some(mode) = mode {
            command.args(["--progress", mode]);
        }
        let (output, terminal) = terminal_output(command, stdout_tty, stderr_tty);
        assert!(output.status.success(), "{output:?}\n{terminal}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            terminal.contains(PREFIX) || stderr.contains(PREFIX),
            expected,
            "{stdout_tty}/{stderr_tty}/{ci}/{mode:?}: {terminal}\n{stderr}"
        );
        if expected {
            assert!(terminal.find("Starting Main").unwrap() < terminal.find("APP_LOG").unwrap());
        }
    }
}

#[test]
fn failures_name_the_last_phase_and_never_claim_an_unstarted_main() {
    for (source, phase, main_started) in [
        (
            "fn main() { let value: Int = true; }",
            "Checking application types",
            false,
        ),
        (
            "fn main() { let value = ; }",
            "Parsing application sources",
            false,
        ),
        (
            "fn main() { panic(\"application failed\"); }",
            "Running Main",
            true,
        ),
    ] {
        let fixture = Fixture::new(source);
        let output = fixture.command().arg("--progress=always").output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        let rows = progress_rows(&stderr);
        assert_eq!(
            rows.iter()
                .any(|(_, text)| text.starts_with("Starting Main")),
            main_started
        );
        assert!(
            rows.last()
                .unwrap()
                .1
                .starts_with(&format!("Failed during {phase}: exit=1")),
            "{stderr}"
        );
    }
    let fixture = Fixture::new("fn main() { panic(\"application failed\"); }");
    let output = fixture.command().arg("--progress=never").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains(PREFIX));
    assert!(stderr.contains("application failed"), "{stderr}");
}

#[test]
fn dependency_names_and_frontend_stages_follow_cold_warm_and_changed_inputs() {
    let fixture = Fixture::dependency_project();
    for (attempt, expected_stdout) in [b"42\n", b"42\n", b"43\n"].into_iter().enumerate() {
        if attempt == 2 {
            std::fs::write(
                fixture.0.path().join("foundation/src/lib.rinzu"),
                "fn answer()->Int{return 41;}export answer;",
            )
            .unwrap();
        }
        let output = fixture
            .command_for("app")
            .arg("--progress=always")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, expected_stdout);
        let stderr = String::from_utf8(output.stderr).unwrap();
        let rows = progress_rows(&stderr);
        let position = |phase| rows.iter().position(|(_, text)| *text == phase);
        for phase in [
            "Resolving dependencies",
            "Checking dependency: foundation",
            "Checking dependency: middle",
            "Parsing application sources",
            "Checking application types",
            "Preparing and verifying IR",
            "Preparing frontend cache",
        ] {
            assert_eq!(position(phase).is_some(), attempt != 1, "{stderr}");
        }
        assert_eq!(
            position("Validating cached inputs").is_some(),
            attempt != 0,
            "{stderr}"
        );
        assert_eq!(
            position("Verifying cached IR").is_some(),
            attempt == 1,
            "{stderr}"
        );
        if attempt != 1 {
            assert!(
                position("Checking dependency: foundation")
                    < position("Checking dependency: middle")
            );
            assert!(
                position("Checking application types") < position("Preparing and verifying IR")
            );
        } else {
            assert!(
                !rows
                    .iter()
                    .any(|(_, text)| text.starts_with("Checking dependency:"))
            );
            assert!(!stderr.contains("Waiting for native dependencies"));
        }
    }
}

#[test]
fn dependency_errors_keep_the_package_name_and_skip_unreached_application_stages() {
    let fixture = Fixture::dependency_project();
    std::fs::write(
        fixture.0.path().join("foundation/src/lib.rinzu"),
        "fn answer()->Int{return true;}export answer;",
    )
    .unwrap();
    for mode in ["always", "auto", "never"] {
        let output = fixture
            .command_for("app")
            .arg(format!("--progress={mode}"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        let rows = progress_rows(&stderr);
        if mode == "always" {
            assert!(
                rows.iter()
                    .any(|(_, text)| *text == "Checking dependency: foundation")
            );
            assert!(
                rows.last()
                    .unwrap()
                    .1
                    .starts_with("Failed during Checking dependency: foundation: exit=1"),
                "{stderr}"
            );
            for phase in [
                "Checking dependency: middle",
                "Parsing application sources",
                "Checking application types",
                "Preparing and verifying IR",
                "Preparing frontend cache",
                "Waiting for native dependencies",
                "Starting Main",
            ] {
                assert!(!stderr.contains(phase), "{stderr}");
            }
        } else {
            assert!(rows.is_empty(), "{stderr}");
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn unfinished_native_preparation_is_reported_before_waiting_for_it() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;

    if !std::thread::available_parallelism().is_ok_and(|count| count.get() > 1) {
        return;
    }
    let fixture = Fixture::dependency_project();
    let root = fixture.0.path();
    let manifest = root.join("foundation/rinzu.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        format!("{text}\n[native]\nsources=[\"answer.c\"]\nheaders=[\"answer.h\"]\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("foundation/src/lib.rinzu"),
        "native{fn answer()->Int symbol \"progress_answer\";}export answer;",
    )
    .unwrap();
    std::fs::write(
        root.join("foundation/answer.h"),
        "#include <stdint.h>\nint64_t progress_answer(void);\n",
    )
    .unwrap();
    std::fs::write(
        root.join("foundation/answer.c"),
        "#include <stdint.h>\nint64_t progress_answer(void){return 40;}\n",
    )
    .unwrap();
    let clang = root.join("clang-gate");
    std::fs::write(
        &clang,
        r#"#!/bin/sh
compile=0
answer=0
for argument in "$@"; do
    case "$argument" in
        -c) compile=1 ;;
        */answer.c) answer=1 ;;
    esac
done
if [ "$compile" = 1 ] && [ "$answer" = 1 ]; then
    while [ ! -f "$RINZU_PROGRESS_RELEASE" ]; do sleep 0.01; done
fi
exec /usr/bin/clang "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&clang, std::fs::Permissions::from_mode(0o700)).unwrap();
    let release = root.join("release-native");
    let mut child = fixture
        .command_for("app")
        .arg("--progress=always")
        .env("RINZU_CLANG", &clang)
        .env("RINZU_PROGRESS_RELEASE", &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut log = String::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.unwrap();
            if line.starts_with(PREFIX) && line.ends_with("Waiting for native dependencies") {
                let _ = send.send(());
            }
            log.push_str(&line);
            log.push('\n');
        }
        log
    });
    let waiting = receive.recv_timeout(Duration::from_secs(30)).is_ok();
    // Release the compiler even on a missing progress event, so all child
    // processes settle before the fixture is removed or an assertion fails.
    std::fs::write(&release, b"continue").unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = reader.join().unwrap();
    assert!(output.status.success(), "{output:?}\n{stderr}");
    assert_eq!(output.stdout, b"42\n");
    assert!(waiting, "{stderr}");
    let rows = progress_rows(&stderr);
    let waiting = rows
        .iter()
        .position(|(_, text)| *text == "Waiting for native dependencies")
        .unwrap();
    let caching = rows
        .iter()
        .position(|(_, text)| *text == "Preparing frontend cache")
        .unwrap();
    assert!(waiting < caching, "{stderr}");
}

#[test]
fn application_exit_status_is_reported_without_changing_it() {
    let fixture = Fixture::new(
        r#"
import rinzu.process;
fn main() {
    if !rinzu.process.setExitStatus(7) { panic("invalid exit status"); }
}
"#,
    );
    let output = fixture.command().arg("--progress=always").output().unwrap();
    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        progress_rows(&stderr)
            .last()
            .unwrap()
            .1
            .starts_with("Finished: exit=7, execution=")
    );
}
