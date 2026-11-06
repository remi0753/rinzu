#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rinzu-{label}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(frozen_cli::path());
        command
            .current_dir(&self.0)
            .env("RINZU_CACHE_DIR", self.0.join("cache"))
            .env("RINZU_GC_STRESS", "1");
        command
    }

    fn build(&self, input: &Path) -> PathBuf {
        let executable = self.0.join("app");
        ok(self
            .cli()
            .arg("build")
            .arg(input)
            .arg("--output")
            .arg(&executable));
        executable
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn ok(command: &mut Command) -> Output {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

const ARGUMENTS: &str = include_str!("fixtures/process_arguments.rinzu");
const JSON: &str = include_str!("fixtures/json_operations.rinzu");

#[test]
fn raw_program_arguments_cross_all_launch_boundaries() {
    let scratch = Scratch::new("arguments-test");
    let path = scratch.0.join("entry.rinzu");
    fs::write(&path, ARGUMENTS).unwrap();
    let values = [
        "取引 11月.json",
        "",
        " ",
        "\n",
        "--",
        "-L",
        "@file",
        "--resource-report",
    ];
    let expected = values
        .iter()
        .map(|value| format!("<{value}>\n"))
        .collect::<String>();
    assert_eq!(ok(scratch.cli().arg("run").arg(&path)).stdout, b"");
    assert_eq!(
        ok(scratch.cli().arg("run").arg(&path).arg("--").args(values)).stdout,
        expected.as_bytes(),
    );
    let executable = scratch.build(&path);
    assert_eq!(ok(&mut Command::new(&executable)).stdout, b"");
    assert_eq!(
        ok(Command::new(&executable).args(values)).stdout,
        expected.as_bytes()
    );
    let invalid = OsString::from_vec(b"ok\xed\xa0\x80".to_vec());
    for output in [
        ok(scratch
            .cli()
            .arg("run")
            .arg(&path)
            .args(["--", "first"])
            .arg(&invalid)),
        ok(Command::new(executable).arg("first").arg(&invalid)),
    ] {
        assert_eq!(output.stdout, b"invalid 1/2\n");
    }
    ok(scratch
        .cli()
        .arg("verify-aot")
        .arg(&path)
        .args(["--", "first"])
        .arg(&invalid));
    let bad_option = scratch.cli().arg("run").arg(&invalid).output().unwrap();
    assert!(!bad_option.status.success());
    assert!(String::from_utf8_lossy(&bad_option.stderr).contains("must be UTF-8"));

    let app = scratch.0.join("package");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("rinzu.toml"),
        "[package]\nname=\"arguments\"\nlanguage=\"0.5\"\nentry=\"src/main.rinzu\"\n",
    )
    .unwrap();
    let source = ARGUMENTS.replace("fn main()", "async fn main()");
    fs::write(app.join("src/main.rinzu"), source).unwrap();
    ok(scratch.cli().arg("lock").arg(&app));
    let lock = fs::read(app.join("rinzu.lock")).unwrap();
    assert_eq!(
        ok(scratch
            .cli()
            .arg("dev")
            .arg(&app)
            .args(["--once", "--"])
            .args(values))
        .stdout,
        expected.as_bytes(),
    );
    assert_eq!(ok(scratch.cli().arg("test").arg(&app)).stdout, b"");
    ok(scratch
        .cli()
        .arg("verify-aot")
        .arg(&app)
        .args(["--", "--", "-1"]));
    assert_eq!(fs::read(app.join("rinzu.lock")).unwrap(), lock);
}

#[test]
fn json_values_and_schema_diagnostics_match_across_runtimes() {
    let scratch = Scratch::new("json-operations-test");
    let path = scratch.0.join("json.rinzu");
    fs::write(&path, JSON).unwrap();
    assert_eq!(ok(scratch.cli().arg("run").arg(&path)).stdout, b"json ok\n");
    ok(scratch.cli().arg("verify-aot").arg(&path));

    let native = JSON.replace(
        "fn main() {",
        "async fn main() { let open = rinzu.io.liveFiles();",
    );
    fs::write(&path, format!("import rinzu.io;\n{native}")).unwrap();
    let result = ok(scratch.cli().arg("run").arg(&path));
    assert_eq!(result.stdout, b"json ok\n");
    assert!(String::from_utf8_lossy(&result.stderr).contains("compiled JIT"));
    ok(scratch.cli().arg("verify-aot").arg(&path));

    fs::write(
        &path,
        r#"
import rinzu.json;
import rinzu.list;
import rinzu.process;
fn worker(args: List<String>, data: ByteArray) -> Int {
    try {
        let values = rinzu.json.parseWithTy<List<Int>>(data);
        let parsed = rinzu.json.parse(data);
        return rinzu.list.length(args) + rinzu.list.length(values) + rinzu.json.arrayLength(parsed);
    }
    catch errors { return -1; }
}
async fn main() {
    try {
        let args = rinzu.process.arguments();
        let value = await spawn worker(args, byteArray(b"[3,5]"));
        if value != 5 { panic("Worker did not receive the arguments and JSON"); }
    }
    catch errors { panic("Worker failed"); }
}
"#,
    )
    .unwrap();
    ok(scratch
        .cli()
        .arg("verify-aot")
        .arg(&path)
        .args(["--", "shared"]));

    fs::write(
        &path,
        r#"
import rinzu.json;
class Unsupported { value: Int }
fn main() {
    try {
        let invalid: { nested: List<Unsupported> } = rinzu.json.parseStringWithTy("{}");
    }
    catch errors {}
}
"#,
    )
    .unwrap();
    let invalid = scratch.cli().arg("check").arg(&path).output().unwrap();
    assert!(!invalid.status.success());
    let diagnostic = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        diagnostic.contains("unsupported JSON target type") && diagnostic.contains("nested"),
        "{diagnostic}"
    );

    fs::write(
        &path,
        r#"
import rinzu.process;
fn readArguments() {
    try { let values = rinzu.process.arguments(); } catch errors {}
}
async fn main() {
    let done = await spawn readArguments();
}
"#,
    )
    .unwrap();
    let rejected = scratch.cli().arg("check").arg(&path).output().unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("Worker"),
        "{rejected:?}"
    );
}

#[test]
fn transaction_report_closes_files_and_returns_the_requested_status() {
    let scratch = Scratch::new("json-report-test");
    let report =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/programs/json-report");
    let package = scratch.0.join("report");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::copy(report.join("rinzu.toml"), package.join("rinzu.toml")).unwrap();
    fs::copy(
        report.join("src/main.rinzu"),
        package.join("src/main.rinzu"),
    )
    .unwrap();
    fs::write(
        scratch.0.join("取引 11月.json"),
        r#"[{"amount":1200},{"amount":330}]"#,
    )
    .unwrap();
    fs::write(scratch.0.join("syntax.json"), "[0,]").unwrap();
    fs::write(scratch.0.join("schema.json"), r#"[{"amount":null}]"#).unwrap();
    let executable = scratch.build(&package);
    for (args, status, output, diagnostic) in [
        (vec!["取引 11月.json"], 0, "件数=2 合計=1530\n", ""),
        (vec![], 2, "", "usage: report FILE"),
        (vec!["one", "two"], 2, "", "usage: report FILE"),
        (vec!["missing"], 1, "", "cannot read or report transactions"),
        (vec!["syntax.json"], 1, "", "invalid JSON at byte 3"),
        (vec!["schema.json"], 1, "", "invalid transaction schema"),
    ] {
        let mut aot = Command::new(&executable);
        aot.current_dir(&scratch.0).env("RINZU_GC_STRESS", "1");
        for result in [
            scratch
                .cli()
                .arg("run")
                .arg(&package)
                .arg("--")
                .args(&args)
                .output()
                .unwrap(),
            aot.args(&args).output().unwrap(),
        ] {
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.code(), Some(status), "{stderr}");
            assert_eq!(result.stdout, output.as_bytes());
            assert!(stderr.contains(diagnostic), "{stderr}");
        }
        ok(scratch
            .cli()
            .arg("verify-aot")
            .arg(&package)
            .arg("--")
            .args(&args));
    }
}
