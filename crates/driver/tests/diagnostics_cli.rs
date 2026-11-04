#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str, raw: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rinzu-diagnostic-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::write(
            path.join("rinzu.toml"),
            "[package]\nname=\"diagnostic_app\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        std::fs::write(path.join("src/main.rinzu"), source).unwrap();
        let fixture = Self(path);
        // Raw BOM/CRLF is intentional provenance coverage, not an authored style example.
        if !raw {
            fixture.format();
        }
        fixture
    }
    fn format(&self) {
        let output = Command::new(frozen_cli::path())
            .args(["fmt", "--write"])
            .arg(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn run(&self, flags: &[&str]) -> Output {
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&self.0)
            .args(flags)
            .output()
            .unwrap()
    }
    fn diagnose(&self, flags: &[&str]) -> Output {
        Command::new(frozen_cli::path())
            .arg("diagnose")
            .arg(self.0.join("run.json"))
            .args(flags)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }
    fn read(&self, file: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.0.join(file)).unwrap()).unwrap()
    }
    fn analysis(&self, flags: &[&str]) -> Value {
        let mut flags = flags.to_vec();
        flags.extend(["--format", "json"]);
        let output = self.diagnose(&flags);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn report_path(&self) -> PathBuf {
        self.0.join("run.json")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn text(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn captured_bom_crlf_unicode_failure_does_not_read_changed_source() {
    let source = "\u{feff}// 日本語🙂\r\nfn main() {\r\n    panic(\"failed\");\r\n}\r\n";
    let fixture = Fixture::new(source, true);
    let report = fixture.report_path();
    let resources = fixture.0.join("resources.jsonl");
    let output = fixture.run(&[
        "--diagnostic-report",
        text(&report),
        "--resource-report",
        text(&resources),
        "--resource-interval-ms",
        "0",
    ]);
    assert!(!output.status.success());
    let captured = fixture.read("run.json");
    assert_eq!(captured["phase"], "finished");
    assert_eq!(captured["sources"][0]["text"], source);
    assert_eq!(captured["outcome"]["failure"]["primary"]["cause"], "Panic");
    std::fs::write(fixture.0.join("src/main.rinzu"), "fn main() {\n}\n").unwrap();
    let analysis = fixture.analysis(&[]);
    let failure = &analysis["outcome"]["failure"]["primary"];
    assert_eq!(failure["location"]["line"], 3);
    assert_eq!(failure["location"]["column"], 5);
    assert_eq!(failure["location"]["strength"], "captured");
    assert_eq!(analysis["resources"]["recordedRunEndSeen"], true);
    assert!(
        analysis["resources"]["categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["latest"]["id"] == "sdk_memory"
                && row["latest"]["status"] == "unknown"
                && row["latest"]["live"].is_null())
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&report).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read_to_string(&report).unwrap().chars().last(),
        Some('\n')
    );
}

#[test]
fn default_output_status_and_stdout_protocol_lookalikes_are_preserved() {
    let fixture = Fixture::new(
        &checked_entry::source(
            "import rinzu.stdio;\nfn main() {\n    rinzu.stdio.writeLine(\"RINZU_FAILURE=conversion\");\n}\n",
        ),
        false,
    );
    let normal = fixture.run(&[]);
    let report = fixture.report_path();
    let captured = fixture.run(&["--diagnostic-report", text(&report)]);
    assert!(
        normal.status.success(),
        "{}",
        String::from_utf8_lossy(&normal.stderr)
    );
    assert_eq!(normal.status.code(), captured.status.code());
    assert_eq!(normal.stdout, captured.stdout);
    let analysis = fixture.analysis(&[]);
    assert_eq!(analysis["outcome"]["status"], "pass");
    assert!(analysis["outcome"]["failure"].is_null());
    assert_eq!(analysis["outcome"]["settlement"], "confirmed");
}

#[test]
fn output_aliases_inputs_and_duplicate_or_wrong_command_flags_are_rejected() {
    let fixture = Fixture::new("fn main() {\n}\n", false);
    let source = fixture.0.join("src/main.rinzu");
    let original = std::fs::read(&source).unwrap();
    let alias = fixture.0.join("alias.rinzu");
    std::fs::hard_link(&source, &alias).unwrap();
    for path in [&source, &alias, &fixture.0.join("src/../rinzu.toml")] {
        let output = fixture.run(&["--diagnostic-report", text(path)]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("collides"));
    }
    let parent = fixture.0.join("linked");
    std::os::unix::fs::symlink(&fixture.0, &parent).unwrap();
    let report = fixture.report_path();
    let aliased_report = parent.join("sub/../run.json");
    let output = fixture.run(&[
        "--diagnostic-report",
        text(&report),
        "--resource-report",
        text(&aliased_report),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("distinct"));
    assert!(
        !fixture
            .run(&[
                "--diagnostic-report",
                text(&report),
                "--diagnostic-report",
                text(&report)
            ])
            .status
            .success()
    );
    assert!(
        !Command::new(frozen_cli::path())
            .arg("build")
            .arg(&fixture.0)
            .arg("--diagnostic-report")
            .arg(&report)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(std::fs::read(source).unwrap(), original);
}

#[test]
fn legacy_activity_and_external_exit_do_not_invent_logical_ids_or_completion() {
    let fixture = Fixture::new("fn main() {\n}\n", false);
    let report = fixture.report_path();
    assert!(
        fixture
            .run(&["--diagnostic-report", text(&report)])
            .status
            .success()
    );
    let mut initial = fixture.read("run.json");
    initial["phase"] = "prepared".into();
    initial["outcome"] = Value::Null;
    initial["exitCode"] = Value::Null;
    std::fs::write(&report, serde_json::to_vec(&initial).unwrap()).unwrap();
    std::fs::write(fixture.0.join("native.log"),"rinzu warning: shutdown-blocker symbol=block source=1:0:2 owner=0x123 depth=1 elapsed_ms=5001 activity=2 stack=block>main\nunknown partial notification\n").unwrap();
    std::fs::write(fixture.0.join("exit.json"),serde_json::to_vec(&serde_json::json!({"schema":"rinzu.process-exit","version":1,"processId":initial["processId"],"exitCode":86,"signal":null,"observer":"test parent wait"})).unwrap()).unwrap();
    let analysis = fixture.analysis(&["--native-log", "native.log", "--exit-record", "exit.json"]);
    assert_eq!(analysis["outcome"]["status"], "unknown");
    assert_eq!(analysis["outcome"]["settlement"], "unconfirmed");
    assert_eq!(
        analysis["native"]["events"][0]["association"],
        "legacy-candidate"
    );
    assert!(analysis["native"]["events"][0]["logical"]["owner"].is_null());
    assert_eq!(
        analysis["native"]["events"][0]["physical"]["pthreadOwnerTag"],
        "0x123"
    );
    assert_eq!(analysis["native"]["events"][1]["parsed"], false);
    assert_eq!(analysis["externalExit"]["processMatches"], true);
    initial["sources"][0]["text"] = "tampered".into();
    std::fs::write(&report, serde_json::to_vec(&initial).unwrap()).unwrap();
    assert!(!fixture.diagnose(&[]).status.success());
}

#[test]
fn source_capture_omission_and_native_analysis_budgets_are_explicit() {
    let source = format!("// {}\nfn main() {{\n}}\n", "x".repeat(1024 * 1024));
    let fixture = Fixture::new(&source, true);
    let report = fixture.report_path();
    assert!(
        fixture
            .run(&["--diagnostic-report", text(&report)])
            .status
            .success()
    );
    let captured = fixture.read("run.json");
    assert!(captured["sources"][0]["text"].is_null());
    assert!(captured["sources"][0]["digest"].is_string());
    let line = "rinzu warning: long-native symbol=n source=1:1:2 owner=0x123 depth=1 elapsed_ms=51 activity=2 stack=n\n";
    std::fs::write(fixture.0.join("native.log"), line.repeat(100000)).unwrap();
    let analysis = fixture.analysis(&["--native-log", "native.log"]);
    assert_eq!(analysis["native"]["events"].as_array().unwrap().len(), 4096);
    assert!(analysis["native"]["omittedEvents"].as_u64().unwrap() > 0);
    assert_eq!(analysis["native"]["input"]["truncated"], true);
    assert_eq!(
        analysis["native"]["events"][0]["location"]["strength"],
        "unresolved"
    );
}

#[test]
fn compiled_private_failure_and_native_activity_use_runtime_source_mapping() {
    let fixture = Fixture::new(
        "import z_delay;\nasync fn main() {\n    let delayed = z_delay.pause(150);\n    panic(\"failed\");\n}\n",
        false,
    );
    let sdk = fixture.0.join("delay");
    std::fs::create_dir_all(sdk.join("src")).unwrap();
    std::fs::write(sdk.join("rinzu.toml"),"[package]\nname=\"z_delay\"\nkind=\"sdk\"\nlanguage=\"0.5\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"native.c\"]\n").unwrap();
    std::fs::write(sdk.join("src/api.rinzu"),"native {\n    fn delay(ms: Int) -> Int symbol \"diagnostic_delay\";\n}\nfn pause(ms: Int) -> Int {\n    return delay(ms);\n}\nexport pause;\n").unwrap();
    std::fs::write(sdk.join("native.c"),"#include <stdint.h>\n#include <unistd.h>\nint64_t diagnostic_delay(int64_t ms) { usleep((useconds_t)(ms*1000)); return ms; }\n").unwrap();
    let mut manifest = std::fs::read_to_string(fixture.0.join("rinzu.toml")).unwrap();
    manifest.push_str("[dependencies]\nz_delay={path=\"delay\"}\n");
    std::fs::write(fixture.0.join("rinzu.toml"), manifest).unwrap();
    fixture.format();
    let output = Command::new(frozen_cli::path())
        .arg("run")
        .arg(&fixture.0)
        .arg("--diagnostic-report")
        .arg(fixture.report_path())
        .arg("--resource-report")
        .arg(fixture.0.join("resources.jsonl"))
        .arg("--resource-interval-ms")
        .arg("0")
        .env("RINZU_DIAGNOSTIC_OUTPUT", fixture.0.join("native.log"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report = fixture.read("run.json");
    assert_eq!(
        report["outcome"]["engine"],
        "compiled-jit",
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["outcome"]["failure"]["primary"]["cause"], "Panic");
    assert_eq!(report["outcome"]["traceSummary"]["eventCount"], 0);
    let main = &report["sources"][0];
    assert_eq!(main["path"], "diagnostic_app/src/main.rinzu");
    assert_ne!(main["file"], main["parseFile"]);
    assert!(report["sources"][1]["text"].is_string());
    let analysis = fixture.analysis(&["--native-log", "native.log"]);
    assert_eq!(
        analysis["outcome"]["failure"]["primary"]["location"]["path"],
        "diagnostic_app/src/main.rinzu"
    );
    assert_eq!(
        analysis["outcome"]["failure"]["primary"]["location"]["line"],
        std::fs::read_to_string(fixture.0.join("src/main.rinzu"))
            .unwrap()
            .lines()
            .position(|line| line.contains("panic(\"failed\")"))
            .unwrap()
            + 1
    );
    assert_eq!(analysis["resources"]["recordedRunEndSeen"], true);
    let events = analysis["native"]["events"].as_array().unwrap();
    assert!(events.iter().any(|event| event["kind"] == "long-native"
        && event["location"]["path"] == "z_delay/src/api.rinzu"
        && event["location"]["strength"] == "candidate"));
    assert!(events.iter().any(|event| event["kind"] == "long-managed"
        && event["location"]["path"] == "diagnostic_app/src/main.rinzu"));
}

#[test]
fn primary_cleanup_and_async_failures_keep_their_source_and_exit_status() {
    for source in [
        "fn main() {\n    try {\n        panic(\"primary\");\n    } finally {\n        let cleanup = CInt8(400);\n    }\n}\n",
        "async fn main() {\n    try {\n        panic(\"primary\");\n    } finally {\n        let cleanup = CInt8(400);\n    }\n}\n",
    ] {
        let fixture = Fixture::new(&checked_entry::source(source), false);
        let report = fixture.report_path();
        let output = fixture.run(&["--diagnostic-report", text(&report)]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let analysis = fixture.analysis(&[]);
        assert_eq!(analysis["observedExitCode"], 1);
        let failure = &analysis["outcome"]["failure"];
        let formatted = std::fs::read_to_string(fixture.0.join("src/main.rinzu")).unwrap();
        let line = |token: &str| {
            formatted
                .lines()
                .position(|line| line.contains(token))
                .unwrap()
                + 1
        };
        assert_eq!(
            failure["primary"]["location"]["line"],
            line("panic(\"primary\")")
        );
        assert_eq!(
            failure["cleanup"][0]["location"]["line"],
            line("let cleanup")
        );
        assert_eq!(analysis["outcome"]["settlement"], "confirmed");
    }
}

#[test]
fn mixed_resource_contexts_invalid_ranges_and_strict_trace_schema_are_visible() {
    let fixture = Fixture::new("fn main() {\n}\n", false);
    let report = fixture.report_path();
    let resources = fixture.0.join("resources.jsonl");
    let output = fixture.run(&[
        "--trace",
        "--diagnostic-report",
        text(&report),
        "--resource-report",
        text(&resources),
        "--resource-interval-ms",
        "0",
    ]);
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let trace = &stderr[stderr.find('{').unwrap()..=stderr.rfind('}').unwrap()];
    std::fs::write(fixture.0.join("trace.json"), trace).unwrap();
    let valid = fixture.analysis(&["--trace", "trace.json"]);
    assert_eq!(valid["trace"]["eventCount"], 2);
    assert_eq!(valid["trace"]["association"], "independent-trace-candidate");
    let mut trace: Value = serde_json::from_str(trace).unwrap();
    trace["events"][0]["inventedTaskId"] = "unpublished".into();
    std::fs::write(
        fixture.0.join("trace.json"),
        serde_json::to_vec(&trace).unwrap(),
    )
    .unwrap();
    assert!(
        !fixture
            .diagnose(&["--trace", "trace.json"])
            .status
            .success()
    );
    let original = std::fs::read_to_string(&resources).unwrap();
    let mut sample: Value = serde_json::from_str(original.lines().last().unwrap()).unwrap();
    sample["process_id"] = 1.into();
    sample["categories"][0]["attribution"] = serde_json::json!({"source_file":1,"start_byte":0,"end_byte":2,"symbol":"main","precision":"site"});
    let mut mismapped = sample.clone();
    mismapped["sources"]["1"] = "other/source.rinzu".into();
    mismapped["categories"][0]["attribution"]["start_byte"] = 900.into();
    mismapped["categories"][0]["attribution"]["end_byte"] = 901.into();
    std::fs::write(
        &resources,
        format!("{original}{}\n{}\nmalformed\n", sample, mismapped),
    )
    .unwrap();
    let analysis = fixture.analysis(&[]);
    assert_eq!(analysis["resources"]["mismatchedContexts"], 2);
    assert_eq!(analysis["resources"]["invalidRecordsOrRows"], 1);
    let rows = analysis["resources"]["categories"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["latest"]["association"] == "legacy-candidate"
                && row["latest"]["location"]["strength"] == "candidate")
    );
    assert!(
        rows.iter()
            .any(|row| row["latest"]["association"] == "legacy-candidate"
                && row["latest"]["location"]["reason"]
                    == "source body omitted or invalid byte/UTF-8 boundary")
    );
    std::fs::remove_file(&resources).unwrap();
    assert!(fixture.analysis(&[])["resources"]["unavailable"].is_string());
    let mut invalid = fixture.read("run.json");
    invalid["outcome"] = serde_json::json!({"failure":{"primary":1,"cleanup":[]}});
    std::fs::write(&report, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(!fixture.diagnose(&[]).status.success());
}

#[test]
fn resource_site_budget_and_fifo_inputs_do_not_hide_limits_or_block() {
    let fixture = Fixture::new("fn main() {\n}\n", false);
    let resources = fixture.0.join("resources.jsonl");
    assert!(
        fixture
            .run(&[
                "--diagnostic-report",
                text(&fixture.report_path()),
                "--resource-report",
                text(&resources),
                "--resource-interval-ms",
                "0"
            ])
            .status
            .success()
    );
    let mut sample: Value = serde_json::from_str(
        std::fs::read_to_string(&resources)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    let base = sample["categories"][0].clone();
    sample["categories"]=serde_json::json!((0..5000).map(|index| {
        let mut row=base.clone();
        row["attribution"]=serde_json::json!({"source_file":1,"start_byte":0,"end_byte":2,"symbol":format!("site{index}"),"precision":"site"});
        row
    }).collect::<Vec<_>>());
    std::fs::write(&resources, serde_json::to_vec(&sample).unwrap()).unwrap();
    let analysis = fixture.analysis(&[]);
    assert_eq!(
        analysis["resources"]["categories"]
            .as_array()
            .unwrap()
            .len(),
        4096
    );
    assert_eq!(analysis["resources"]["omittedRowObservations"], 904);
    let fifo = fixture.0.join("fifo");
    let path = std::ffi::CString::new(text(&fifo)).unwrap();
    // SAFETY: NUL-terminated path names this fixture's writable temporary directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let mut child = Command::new(frozen_cli::path())
        .arg("diagnose")
        .arg(fixture.report_path())
        .args(["--native-log", text(&fifo)])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success());
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("FIFO input blocked diagnose");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
