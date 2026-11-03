#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_callback_return_sdk/faults.rs"]
mod faults;
#[path = "fixtures/native_callback_return_sdk/setup.rs"]
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
fn baseline(output: &Output, owners: usize, extra: usize, caught_outer: usize) {
    let stdout = String::from_utf8_lossy(&output.stderr);
    let calls = numbers(&stdout, "RINZU_NATIVE_CALLS=");
    assert_eq!(&calls[..6], &[70 * owners + extra; 6]);
    assert_eq!(&calls[6..8], &[56 * owners, 160 * owners]);
    assert_eq!(&calls[8..], &[0; 3]);
    assert_eq!(&numbers(&stdout, "RINZU_NATIVE_ENTRIES=")[4..], &[0, 0]);
    assert_eq!(
        numbers(&stdout, "RINZU_CALLBACK_RETURNS="),
        [24 * owners, 12 * owners, 266 * owners, 864 * owners]
    );
    let catches = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_CAUGHT_ERRORS="))
        .unwrap();
    assert_eq!(catches.split(';').count(), 8 * owners + caught_outer);
    assert!(
        catches
            .split(';')
            .all(|diagnostic| diagnostic.starts_with("5@"))
    );
}

#[test]
fn callback_return_cli_runs_builds_and_verifies_buffers_and_fallbacks_on_all_owners() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-callback-returns-{}", std::process::id()));
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
    let aot = Command::new(app.join("build/return_app"))
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    success(&aot);
    baseline(&aot, 9, 1, 0);
    success(&cli(&app, "verify-aot"));
    for (main, caught_outer) in [
        (
            "import returns;fn main(){let returned=returns.exercise();if returned!=56 {panic(\"lost\");}}",
            false,
        ),
        (
            "import returns;fn main(){try{try {let invalid=CInt8(300);}finally {let cleaned=returns.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}}",
            true,
        ),
        (
            "import returns;fn fail()->Int {try{try {let invalid=CInt8(300);}finally {let cleaned=returns.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}return 0;}async fn main(){let returned=await spawn fail();}",
            true,
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        let run = cli(&app, "run");
        success(&run);
        success(&cli(&app, "build"));
        let aot = Command::new(app.join("build/return_app"))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&aot);
        baseline(&aot, 1, 0, usize::from(caught_outer));
        success(&cli(&app, "verify-aot"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdk_owned_copy_oom_after_actual_callback_return_is_fatal_on_source_jit_workers_and_aot() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-callback-sdk-copy-fault-{}",
        std::process::id()
    ));
    fixture::create(&root);
    std::fs::write(root.join("returns/api.c"), faults::source()).unwrap();
    let app = root.join("app");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (name, main) in [
        (
            "jit",
            "import returns;fn main(){let returned=returns.exercise();}",
        ),
        (
            "source",
            "import returns;async fn main(){let returned=returns.exercise();}",
        ),
        (
            "worker",
            "import returns;async fn main(){let returned=await spawn returns.exercise();}",
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        success(&cli(&app, "build"));
        let binary = app.join("build/return_app");
        let sanitized = app.join(format!("build/{name}-copy-sanitized"));
        success(
            &Command::new("clang")
                .args([
                    "-std=c11",
                    "-O1",
                    "-g",
                    "-pthread",
                    "-fsanitize=address,undefined",
                    "-fno-omit-frame-pointer",
                ])
                .arg(app.join("build/return_app.o"))
                .arg(app.join("build/return_app.runtime.c"))
                .arg(manifest.join("../runtime/src/callback_entry.c"))
                .arg(manifest.join("../runtime/src/callback_template.S"))
                .arg("-I")
                .arg(manifest.join("../../include"))
                .arg("-o")
                .arg(&sanitized)
                .output()
                .unwrap(),
        );
        for backend in ["run", "aot", "sanitized"] {
            let mut command = if backend == "run" {
                let mut command = Command::new(frozen_cli::path());
                command.arg("run").arg(&app);
                command
            } else {
                Command::new(if backend == "aot" {
                    &binary
                } else {
                    &sanitized
                })
            };
            let output = command
                .env("RINZU_TEST_RETURN_SDK_COPY", "1")
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.code(), Some(71), "{name}/{backend}: {stderr}");
            let entry = stderr.find("RETURN_NATIVE_STRING_ENTER").unwrap();
            let finally = stderr.find("RETURN_FINALLY\n").unwrap();
            let copy = stderr.find("RETURN_SDK_COPY_OOM\n").unwrap();
            assert!(entry < finally && finally < copy, "{stderr}");
            assert!(
                !stdout.contains("RINZU_RESULT=") && !stdout.contains("RINZU_CAUGHT_ERRORS="),
                "{stdout}"
            );
            assert!(
                !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
