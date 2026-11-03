#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_callback_sdk/faults.rs"]
mod faults;
#[path = "fixtures/native_callback_sdk/setup.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

fn cli(app: &Path, command: &str) -> Output {
    Command::new(frozen_cli::path())
        .arg(command)
        .arg(app)
        .output()
        .unwrap()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}
fn numbers(stdout: &str, prefix: &str) -> Vec<usize> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix}: {stdout}"))
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect()
}
fn baseline(output: &Output, owners: usize, caught_outer: usize) {
    let stdout = String::from_utf8_lossy(&output.stderr);
    let calls = numbers(&stdout, "RINZU_NATIVE_CALLS=");
    assert_eq!(&calls[..6], &[79 * owners; 6]);
    assert_eq!(&calls[6..8], &[76 * owners, 884 * owners]);
    assert_eq!(&calls[8..], &[0; 3]);
    assert_eq!(&numbers(&stdout, "RINZU_NATIVE_ENTRIES=")[4..], &[0, 0]);
    let main = numbers(&stdout, "RINZU_NATIVE_USERDATA=");
    assert_eq!(&main[..4], &[0; 4]);
    assert_eq!(main[4], main[5]);
    let mut total = main[4];
    if let Some(workers) = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_WORKER_USERDATA="))
    {
        for worker in workers.split(';').filter(|worker| !worker.is_empty()) {
            let fields = worker
                .split(':')
                .map(|value| value.parse::<usize>().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(&fields[1..5], &[0; 4]);
            assert_eq!(fields[5], fields[6]);
            total += fields[5];
        }
    }
    assert_eq!(total, 72 * owners);
    assert_eq!(numbers(&stdout, "RINZU_USERDATA_PROCESS="), [0, 0, 0]);
    let catches = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_CAUGHT_ERRORS="))
        .unwrap();
    assert_eq!(catches.split(';').count(), 4 * owners + caught_outer);
    assert!(
        catches
            .split(';')
            .all(|diagnostic| diagnostic.starts_with("5@"))
    );
}

#[test]
fn generic_callback_cli_runs_builds_and_verifies_the_common_sdk_on_all_owners() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-generic-callback-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let run = cli(&app, "run");
    success(&run);
    assert!(String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime"));
    assert!(
        app.join(".rinzu/generated/source-packages-v2/manifest.json")
            .is_file()
    );
    success(&cli(&app, "build"));
    let aot = Command::new(app.join("build/callback_app"))
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    success(&aot);
    baseline(&aot, 9, 0);
    success(&cli(&app, "verify-aot"));
    for (main, caught_outer) in [
        (
            "import callbacks;fn main(){let returned=callbacks.exercise();if returned!=callbacks.original(){panic(\"lost\");}}",
            false,
        ),
        (
            "import callbacks;fn main(){try{try {let invalid=CInt8(300);}finally {let cleaned=callbacks.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}}",
            true,
        ),
        (
            "import callbacks;fn fail()->Int {try{try {let invalid=CInt8(300);}finally {let cleaned=callbacks.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}return 0;}async fn main(){let returned=await spawn fail();}",
            true,
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        let run = cli(&app, "run");
        success(&run);
        success(&cli(&app, "build"));
        let aot = Command::new(app.join("build/callback_app"))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&aot);
        baseline(&aot, 1, usize::from(caught_outer));
        success(&cli(&app, "verify-aot"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn generic_callback_sdk_rejects_token_misuse_closed_entries_and_copy_faults_before_body() {
    use std::os::unix::process::ExitStatusExt;
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-generic-callback-faults-{}",
        std::process::id()
    ));
    fixture::create(&root);
    std::fs::write(root.join("callbacks/api.c"), faults::source()).unwrap();
    let app = root.join("app");
    for (name, main) in [
        (
            "jit",
            "import callbacks;fn main(){let returned=callbacks.exercise();}",
        ),
        (
            "source",
            "import callbacks;async fn main(){let returned=callbacks.exercise();}",
        ),
        (
            "worker",
            "import callbacks;async fn main(){let returned=await spawn callbacks.exercise();}",
        ),
        (
            "owners",
            "import callbacks;async fn main(){let one=spawn callbacks.exercise();let two=spawn callbacks.exercise();let r1=await one;let r2=await two;}",
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        success(&cli(&app, "build"));
        let aot = app.join("build/callback_app");
        let sanitizer = root.join(format!("{name}-sanitized"));
        let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
        let linked = Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-omit-frame-pointer",
                "-pthread",
            ])
            .arg(aot.with_file_name("callback_app.o"))
            .arg(aot.with_file_name("callback_app.runtime.c"))
            .arg(native.join("callback_entry.c"))
            .arg(native.join("callback_template.S"))
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg("-o")
            .arg(&sanitizer)
            .output()
            .unwrap();
        success(&linked);
        // Startup's bundle reference is executable-relative.
        let bundle = root.join("callback_app.source-packages");
        if !bundle.exists() {
            std::os::unix::fs::symlink(app.join("build/callback_app.source-packages"), &bundle)
                .unwrap();
        }
        let cases = if name == "owners" {
            vec!["owner"]
        } else {
            vec![
                "type",
                "unknown",
                "scope",
                "scope-closed",
                "callback-closed",
                "thread",
                "bytes-null",
                "bytes-unreadable",
            ]
        };
        for fault in cases {
            for backend in ["run", "aot", "sanitized"] {
                if backend == "sanitized" && fault == "bytes-unreadable" {
                    continue;
                }
                let mut command = if backend == "run" {
                    let mut command = Command::new(frozen_cli::path());
                    command.arg("run").arg(&app);
                    command
                } else {
                    Command::new(if backend == "aot" { &aot } else { &sanitizer })
                };
                let output = command
                    .env("RINZU_TEST_CALLBACK_FAULT", fault)
                    .env("ASAN_OPTIONS", "detect_leaks=0")
                    .output()
                    .unwrap();
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stderr);
                assert!(
                    stderr.contains("CALLBACK_NATIVE_ENTER"),
                    "{name}/{backend}/{fault}: {stderr}"
                );
                if fault == "bytes-unreadable" {
                    assert!(
                        matches!(output.status.signal(), Some(10 | 11)),
                        "{name}/{backend}/{fault}: {:?} {stderr}",
                        output.status
                    );
                } else {
                    assert_eq!(
                        output.status.code(),
                        Some(86),
                        "{name}/{backend}/{fault}: {stderr}"
                    );
                }
                if !["scope", "scope-closed", "callback-closed"].contains(&fault) {
                    assert!(
                        !stderr.contains("CALLBACK_BODY_ENTER"),
                        "{name}/{backend}/{fault}: {stderr}"
                    );
                }
                assert!(
                    !stdout.contains("RINZU_RESULT=") && !stdout.contains("RINZU_CAUGHT_ERRORS="),
                    "{stdout}"
                );
                assert!(
                    !stderr.contains("ERROR: AddressSanitizer")
                        && !stderr.contains("runtime error:"),
                    "{stderr}"
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
