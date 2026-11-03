#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_userdata_sdk/setup.rs"]
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
fn baseline(output: &Output, calls: usize, tokens: usize, callbacks: usize) {
    let stdout = String::from_utf8_lossy(&output.stderr);
    let native = numbers(&stdout, "RINZU_NATIVE_CALLS=");
    assert_eq!(&native[..6], &[calls; 6], "{stdout}");
    assert_eq!(&native[6..8], &[callbacks, callbacks * 4], "{stdout}");
    assert_eq!(&native[8..], &[0, 0, 0], "{stdout}");
    let entries = numbers(&stdout, "RINZU_NATIVE_ENTRIES=");
    assert_eq!(&entries[4..], &[0, 0], "{stdout}");
    let main = numbers(&stdout, "RINZU_NATIVE_USERDATA=");
    assert_eq!(&main[..4], &[0; 4], "{stdout}");
    assert_eq!(main[4], main[5], "{stdout}");
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
            assert_eq!(fields.len(), 7, "{stdout}");
            assert_eq!(&fields[1..5], &[0; 4], "{stdout}");
            assert_eq!(fields[5], fields[6], "{stdout}");
            total += fields[5];
        }
    }
    assert_eq!(total, tokens, "{stdout}");
    assert_eq!(
        numbers(&stdout, "RINZU_USERDATA_PROCESS="),
        [0, 0, 0],
        "{stdout}"
    );
}

#[test]
fn userdata_cli_common_sdk_keeps_argument_count_values_and_owner_baselines() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-userdata-common-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let run = cli(&app, "run");
    success(&run);
    assert!(String::from_utf8_lossy(&run.stderr).contains("compiled JIT continuation runtime"));
    assert!(
        app.join(".rinzu/generated/source-packages-v2/manifest.json")
            .is_file()
    );
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let probe = artifact
        .ir
        .source_native_signatures
        .iter()
        .find(|signature| signature.name.ends_with("::probe"))
        .unwrap();
    assert_eq!(probe.parameters.len(), 7);
    assert_eq!(
        probe
            .parameters
            .iter()
            .filter(|ty| matches!(ty, rinzu_compiler::SourceAbiType::UserData(_)))
            .count(),
        5
    );
    success(&cli(&app, "build"));
    let aot = Command::new(app.join("build/userdata_app"))
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    success(&aot);
    baseline(&aot, 119, 585, 3744);
    success(&cli(&app, "verify-aot"));
    std::fs::write(app.join("src/main.rinzu"),"import userdata; fn main() {let value=userdata.exercise();let returned=userdata.returning();if value!=returned {panic(\"wrong\");}if userdata.preEntryError()!=1 {panic(\"entered\");}}\n").unwrap();
    let run = cli(&app, "run");
    success(&run);
    assert!(String::from_utf8_lossy(&run.stderr).contains("baseline JIT"));
    success(&cli(&app, "build"));
    let aot = Command::new(app.join("build/userdata_app"))
        .output()
        .unwrap();
    success(&aot);
    baseline(&aot, 15, 65, 416);
    success(&cli(&app, "verify-aot"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn userdata_cli_caught_conversion_runs_finally_and_releases_tokens_on_all_owners() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-userdata-uncaught-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let app = root.join("app");
    for (entry, worker) in [
        ("fn main()", false),
        ("async fn main()", false),
        ("async fn main()", true),
    ] {
        let body = if worker {
            "let failed=await spawn userdata.handledFailure();if failed!=1{panic(\"missingFailure\");}"
        } else {
            "try{let failed=userdata.failed();}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"missingFailure\");}}"
        };
        std::fs::write(
            app.join("src/main.rinzu"),
            format!("import userdata; {entry} {{{body}}}\n"),
        )
        .unwrap();
        let run = cli(&app, "run");
        success(&run);
        success(&cli(&app, "build"));
        let aot = Command::new(app.join("build/userdata_app"))
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&aot);
        let stdout = String::from_utf8_lossy(&aot.stderr);
        assert!(
            stdout.contains("RINZU_CAUGHT_ERRORS=5@")
                && !stdout.contains("RINZU_FAILURE=conversion"),
            "{stdout}"
        );
        baseline(&aot, 2, 10, 64);
        success(&cli(&app, "verify-aot"));
    }
    std::fs::remove_dir_all(root).unwrap();
}
