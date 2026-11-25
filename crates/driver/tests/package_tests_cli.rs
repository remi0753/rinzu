#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn published_rinzu_documentation_examples_pass() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = Command::new("git")
        .args(["ls-files", "-z", "docs"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(files.status.success(), "{files:?}");
    let standard = Fixture::new("fn main() {}\n");
    let codec = Fixture::new("fn main() {}\n");
    codec.dependency("codec");
    let mut cases = 0;
    for name in std::str::from_utf8(&files.stdout)
        .unwrap()
        .split_terminator('\0')
        .filter(|name| name.ends_with(".md"))
    {
        let document = repo.join(name);
        let text = std::fs::read_to_string(&document).unwrap();
        let count = text
            .lines()
            .filter(|line| line.trim() == "```rinzu-test")
            .count();
        if count != 0 {
            cases += count;
            let fixture = if name == "docs/reference/sdk-codec.md" {
                &codec
            } else {
                &standard
            };
            let report = fixture.report(
                &["--timeout-ms", "30000", "--doc", document.to_str().unwrap()],
                true,
            );
            assert_eq!(report["summary"]["passed"], count, "{name}: {report:#}");
        }
    }
    assert!(cases > 0);
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rinzu-file-tests-cli-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("src")).unwrap();
        std::fs::create_dir(path.join("tests")).unwrap();
        std::fs::write(
            path.join("rinzu.toml"),
            "[package]\nname=\"test_app\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        std::fs::write(path.join("src/main.rinzu"), source).unwrap();
        Self(path)
    }
    fn command(&self, path: &str) -> Command {
        let mut command = Command::new(frozen_cli::path());
        command
            .arg("test")
            .arg(self.0.join(path))
            .env("RINZU_CACHE_DIR", self.0.join("user-cache"));
        command
    }
    fn cli(&self, flags: &[&str]) -> Output {
        self.command(".").args(flags).output().unwrap()
    }
    fn file(&self, path: &str, content: &str) -> PathBuf {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
    fn report(&self, flags: &[&str], success: bool) -> Value {
        self.report_at(".", flags, success)
    }
    fn report_at(&self, path: &str, flags: &[&str], success: bool) -> Value {
        let output = self
            .command(path)
            .args(flags)
            .args(["--format", "json"])
            .output()
            .unwrap();
        let report: Option<Value> = serde_json::from_slice(&output.stdout).ok();
        let failures = report.as_ref().map(|report| {
            report["entries"]
                .as_array()
                .or_else(|| report["cases"].as_array())
                .map(|cases| {
                    cases
                        .iter()
                        .map(|case| (&case["identity"], &case["status"], &case["worker"]["error"]))
                        .collect::<Vec<_>>()
                })
        });
        assert_eq!(
            output.status.success(),
            success,
            "{}; case results: {failures:?}",
            String::from_utf8_lossy(&output.stderr),
        );
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    }
    fn dependency(&self, name: &str) {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../sdk")
            .join(name);
        copy_source(&repo, &self.0.join(name));
        if rinzu_compiler::standard_library::registered(&format!("rinzu.{name}")) {
            return;
        }
        let mut text = std::fs::read_to_string(self.0.join("rinzu.toml")).unwrap();
        if !text.contains("[dependencies]") {
            text.push_str("\n[dependencies]\n");
        }
        text.push_str(&format!("\"rinzu.{name}\"={{path=\"{name}\"}}\n"));
        std::fs::write(self.0.join("rinzu.toml"), text).unwrap();
    }
    fn native_sdk() -> Self {
        let fixture = Self::new(
            r#"native {
    fn answer() -> CInt symbol "answer";
    fn burst() symbol "burst";
    fn never() symbol "never";
}
export answer;
export burst;
export never;
"#,
        );
        fixture.file("rinzu.toml","[package]\nname=\"test_native\"\nkind=\"sdk\"\nlanguage=\"0.5\"\nentry=\"src/main.rinzu\"\n[native]\nsources=[\"native/shim.c\"]\nheaders=[\"native/shim.h\"]\n");
        std::fs::create_dir(fixture.0.join("native")).unwrap();
        std::fs::write(
            fixture.0.join("native/shim.h"),
            "int answer(void); void burst(void); void never(void);\n",
        )
        .unwrap();
        std::fs::write(fixture.0.join("native/shim.c"),r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int answer(void){return 42;}
void burst(void){unsigned char data[8192];memset(data,0xa5,sizeof(data));for(int i=0;i<145;i++){fwrite(data,1,sizeof(data),stdout);fwrite(data,1,sizeof(data),stderr);}fflush(stdout);fflush(stderr);}
void never(void){const char *path=getenv("RINZU_TEST_MARKER");if(path){FILE *file=fopen(path,"w");if(file){fprintf(file,"%ld\n",(long)getpid());fclose(file);}}for(;;)pause();}
"#).unwrap();
        fixture
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn copy_source(from: &Path, to: &Path) {
    std::fs::create_dir(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".rinzu" || name == "build" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_source(&entry.path(), &to.join(name));
        } else {
            std::fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}
fn stdout(case: &Value) -> Vec<u8> {
    case["stdoutBytes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect()
}

#[test]
fn package_main_and_test_file_selection_keep_application_artifacts() {
    let fixture = Fixture::new(
        "import rinzu.stdio; fn main(){try{rinzu.stdio.writeLine(\"legacy\");}catch errors{panic(\"failed\");}}",
    );
    fixture.file(
        "src/math.rinzu",
        "fn test_identity<T>(x:T)->T{return x;} export test_identity;",
    );
    fixture.file(
        "tests/a.test.rinzu",
        "import rinzu.stdio; fn main(){rinzu.stdio.writeLine(\"RINZU_RESULT=spoof\");}",
    );
    fixture.file("tests/b.test.rinzu", "async fn main() {}");
    fixture.file(
        "tests/math/型.test.rinzu",
        "import math; fn main(){let value=math.test_identity<Int>(42);let add=fn(x:Int)->Int{return x+1;};if add(value)!=43 {let bad=CInt8(300);}}",
    );
    let original = std::fs::read(fixture.0.join("src/main.rinzu")).unwrap();
    fixture.file("build/keep", "frozen output");
    fixture.file(".rinzu/generated/source-packages-v2/keep", "frozen sidecar");
    let legacy = fixture.cli(&[]);
    assert!(legacy.status.success());
    assert_eq!(legacy.stdout, b"legacy\n");
    let listed = fixture.report_at("tests", &["--list"], true);
    assert_eq!(listed["version"], 2);
    assert_eq!(listed["entries"].as_array().unwrap().len(), 3);
    assert_eq!(listed["summary"]["passed"], 0);
    assert_eq!(listed["entries"][0]["location"], Value::Null);
    assert_eq!(listed["entries"][0]["isAsync"], Value::Null);
    let all = fixture.report_at("tests", &[], true);
    assert_eq!(all["summary"]["passed"], 3);
    assert_eq!(stdout(&all["entries"][0]), b"RINZU_RESULT=spoof\n");
    assert!(
        all["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["settlement"] == "confirmed")
    );
    let selected = fixture.report_at("tests", &["--filter", "型.test"], true);
    assert_eq!(
        selected["entries"][0]["relativePath"],
        "tests/math/型.test.rinzu"
    );
    let selected = fixture.report_at("tests/math", &["--list"], true);
    assert_eq!(selected["summary"]["selected"], 1);
    assert_eq!(
        std::fs::read(fixture.0.join("src/main.rinzu")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(fixture.0.join("build/keep")).unwrap(),
        b"frozen output"
    );
    assert_eq!(
        std::fs::read(fixture.0.join(".rinzu/generated/source-packages-v2/keep")).unwrap(),
        b"frozen sidecar"
    );
}

#[test]
fn discovery_selects_paths_before_parsing_and_skips_nested_packages() {
    let fixture = Fixture::new("invalid source outside the test import graph");
    for index in 0..300 {
        fixture.file(&format!("tests/{index:03}.test.rinzu"), "fn main() {}");
    }
    fixture.file("tests/broken.test.rinzu", "not valid code");
    fixture.file("tests/helper.rinzu", "not a test file");
    fixture.file(
        "tests/fixture/rinzu.toml",
        "[package]\nname=\"nested\"\nlanguage=\"0.5\"\n",
    );
    fixture.file(
        "tests/fixture/tests/ignored.test.rinzu",
        "not part of the parent package",
    );
    std::os::unix::fs::symlink("000.test.rinzu", fixture.0.join("tests/alias.test.rinzu")).unwrap();
    std::os::unix::fs::symlink(
        "fixture/tests/ignored.test.rinzu",
        fixture.0.join("tests/nested-alias.test.rinzu"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "../src/main.rinzu",
        fixture.0.join("tests/outside.test.rinzu"),
    )
    .unwrap();

    let listed = fixture.report_at("tests", &["--list"], true);
    assert_eq!(listed["summary"]["selected"], 301);
    let selected = fixture.report_at("tests", &["--filter", "000.test"], true);
    assert_eq!(selected["summary"]["passed"], 1);
    let alias = fixture.report_at("tests/alias.test.rinzu", &["--list"], true);
    assert_eq!(alias["entries"][0]["identity"], "file:tests/000.test.rinzu");
    let nested = fixture.report_at("tests/nested-alias.test.rinzu", &["--list"], true);
    assert_eq!(nested["package"], "nested");
    assert_eq!(
        nested["entries"],
        fixture.report_at("tests/fixture/tests/ignored.test.rinzu", &["--list"], true)["entries"]
    );
    let broken = fixture.report_at("tests/broken.test.rinzu", &[], false);
    assert_eq!(broken["entries"][0]["status"], "compile-error");
    assert_eq!(broken["entries"][0]["settlement"], "not-run");
    assert_eq!(
        broken["diagnostics"][0]["entryIdentity"],
        "file:tests/broken.test.rinzu"
    );
    let empty = fixture.report_at("tests", &["--filter", "missing"], false);
    assert_eq!(empty["summary"]["selected"], 0);

    assert!(!fixture.cli(&["--unit"]).status.success());
    assert!(
        !fixture
            .command("tests/outside.test.rinzu")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !fixture
            .command("tests/helper.rinzu")
            .output()
            .unwrap()
            .status
            .success()
    );
    for flags in [
        &["--list", "--list"][..],
        &["--filter", ""],
        &["--timeout-ms", "0"],
        &["--format", "xml"],
        &["--resource-report", "report.json"],
        &["--case", "name"],
    ] {
        assert!(
            !fixture
                .command("tests")
                .args(flags)
                .output()
                .unwrap()
                .status
                .success(),
            "{flags:?}"
        );
    }
}

#[test]
fn test_directory_alias_and_physical_path_select_the_same_files() {
    let fixture = Fixture::new("fn main() {}");
    fixture.file("tests/selected.test.rinzu", "fn main() {}");
    std::fs::rename(fixture.0.join("tests"), fixture.0.join("qa")).unwrap();
    std::os::unix::fs::symlink("qa", fixture.0.join("tests")).unwrap();
    let listed = fixture.report_at("tests", &["--list"], true);
    let physical = fixture.report_at("qa/selected.test.rinzu", &["--list"], true);
    assert_eq!(listed["entries"], physical["entries"]);
    assert_eq!(
        listed["entries"][0]["identity"],
        "file:tests/selected.test.rinzu"
    );
    let executed = fixture.report_at("tests", &[], true);
    assert_eq!(executed["summary"]["passed"], 1);
}

#[test]
fn native_link_failure_is_reported_before_main_and_the_next_file_runs() {
    let fixture = Fixture::new("fn main() {}");
    fixture.file(
        "tests/a-link.test.rinzu",
        r#"
import rinzu.stdio;
native { fn missing() symbol "rinzu_test_missing_link_symbol"; }
fn main() {
    rinzu.stdio.writeLine("test main started");
    missing();
}
"#,
    );
    fixture.file("tests/b-pass.test.rinzu", "fn main() {}");
    let report = fixture.report_at("tests", &[], false);
    let failed = &report["entries"][0];
    assert_eq!(failed["status"], "compile-error", "{report:#}");
    assert_eq!(failed["settlement"], "not-run");
    assert!(stdout(failed).is_empty());
    assert!(
        failed["worker"]["error"]
            .as_str()
            .is_some_and(|message| message.contains("rinzu_test_missing_link_symbol")),
        "{report:#}"
    );
    assert_eq!(report["entries"][1]["status"], "pass");
    assert_eq!(report["summary"]["failed"], 1);
}

#[test]
fn production_check_and_build_leave_test_files_unread() {
    let fixture = Fixture::new("fn main() {}");
    fixture.file("tests/broken.test.rinzu", "not valid Rinzu");
    let output = fixture.0.join("app");
    for command in ["check", "build"] {
        let mut cli = Command::new(frozen_cli::path());
        cli.arg(command)
            .arg(&fixture.0)
            .env("RINZU_CACHE_DIR", fixture.0.join("user-cache"));
        if command == "build" {
            cli.arg("--output").arg(&output);
        }
        let result = cli.output().unwrap();
        assert!(result.status.success(), "{result:?}");
    }
    assert!(output.is_file());
}

#[test]
fn executable_doc_examples_are_explicit_and_diagnostics_use_original_document() {
    let fixture = Fixture::new("fn main(){panic(\"bad\");}\n");
    let document = "\u{feff}# 日本語🙂\r\n```rinzu\r\nnot executable\r\n```\r\n```rinzu-test\r\nimport rinzu.stdio;\r\nfn main(){rinzu.stdio.writeLine(\"doc\");}\r\n```\r\n";
    std::fs::write(fixture.0.join("README.md"), document).unwrap();
    let report = fixture.report(&["--doc", "README.md"], true);
    assert_eq!(report["summary"]["passed"], 1);
    assert_eq!(stdout(&report["cases"][0]), b"doc\n");
    assert_eq!(report["cases"][0]["location"]["line"], 7);
    fixture.file("tests/sample.test.rinzu", "fn main() {}");
    let combined = fixture.report_at("tests", &["--doc", "README.md"], true);
    assert_eq!(combined["version"], 2);
    assert_eq!(combined["summary"]["passed"], 2);
    assert_eq!(combined["entries"][0]["kind"], "doc");
    assert_eq!(combined["entries"][1]["kind"], "file");
    std::fs::write(
        fixture.0.join("README.md"),
        "# doc\n```rinzu-test\nfn main(){let value:Int=\"bad\";}\n```\n",
    )
    .unwrap();
    let failed = fixture.report(&["--doc", "README.md"], false);
    assert_eq!(failed["diagnostics"][0]["location"]["line"], 3);
    assert!(
        failed["diagnostics"][0]["location"]["path"]
            .as_str()
            .unwrap()
            .ends_with("README.md")
    );
    std::fs::write(fixture.0.join("README.md"), "```rinzu-test\nfn main() {}\n").unwrap();
    assert_eq!(
        fixture.report(&["--doc", "README.md"], false)["summary"]["passed"],
        0
    );
}

#[test]
fn named_cases_capture_ordered_errors_continue_and_filter_exactly() {
    let fixture = Fixture::new("invalid application source");
    let path = "tests/cases.test.rinzu";
    fixture.file(
        path,
        r#"
import rinzu.testing as testing;
import rinzu.stdio;
import rinzu.timer;

error Cleanup {}

async fn main() {
    rinzu.stdio.writeLine("file main");
    testing.runCase("failure", fn () {
        try {
            testing.assertEqual<Int>(3, 4, "sum");
        }
        finally {
            throw Cleanup(message: "cleanup");
        }
    });
    testing.runCase("-second", fn () {
        rinzu.stdio.writeLine("second case");
    });
    await testing.runAsyncCase("asynchronous 日本語", async fn () {
        await rinzu.timer.sleep(1);
        rinzu.stdio.writeLine("async case");
    });
}
"#,
    );
    let report = fixture.report_at(path, &[], false);
    let entry = &report["entries"][0];
    assert_eq!(entry["status"], "fail", "{report:#}");
    assert_eq!(entry["settlement"], "confirmed");
    assert_eq!(
        report["caseSummary"],
        serde_json::json!({"selected":3,"passed":2,"failed":1})
    );
    let cases = entry["cases"].as_array().unwrap();
    assert_eq!(cases[0]["status"], "fail");
    assert_eq!(cases[0]["errors"][0]["name"], "AssertionFailed");
    assert_eq!(cases[0]["errors"][0]["message"], "sum");
    assert_eq!(cases[0]["errors"][1]["name"], "Cleanup");
    assert_eq!(cases[0]["errors"][1]["message"], "cleanup");
    assert_eq!(cases[1]["status"], "pass");
    assert_eq!(cases[2]["status"], "pass");
    let text = String::from_utf8(stdout(entry)).unwrap();
    assert!(text.contains("actual:\n3\nexpected:\n4\n"));
    assert_eq!(text.matches("file main").count(), 1);
    let single = fixture.report_at(path, &["--case", "asynchronous 日本語"], true);
    let entry = &single["entries"][0];
    assert_eq!(entry["cases"].as_array().unwrap().len(), 1);
    assert_eq!(entry["cases"][0]["name"], "asynchronous 日本語");
    assert_eq!(stdout(entry), b"file main\nasync case\n");
    let second = fixture.report_at(path, &["--case=-second"], true);
    assert_eq!(stdout(&second["entries"][0]), b"file main\nsecond case\n");
    let missing = fixture.report_at(path, &["--case", "asynchronous"], false);
    assert!(
        missing["entries"][0]["worker"]["error"]
            .as_str()
            .unwrap()
            .contains("no named test case")
    );
}

#[test]
fn duplicate_case_names_fail_even_when_the_context_error_is_caught() {
    let fixture = Fixture::new("fn main() {}");
    fixture.file(
        "tests/duplicate.test.rinzu",
        r#"
import rinzu.testing as testing;

fn main() {
    testing.runCase("same", fn () {});
    try {
        testing.runCase("same", fn () {});
    }
    catch [problem:testing.InvalidTestCase] {}
    catch other {panic("unexpected error");}
}
"#,
    );
    let report = fixture.report_at("tests", &[], false);
    let entry = &report["entries"][0];
    assert_eq!(entry["status"], "fail");
    assert_eq!(entry["cases"][0]["status"], "pass");
    assert!(
        entry["worker"]["error"]
            .as_str()
            .unwrap()
            .contains("duplicate test case name")
    );
}

#[test]
fn a_test_main_can_catch_an_assertion_without_a_named_case() {
    let fixture = Fixture::new("fn main() {}");
    fixture.file(
        "tests/assertion.test.rinzu",
        r#"
import rinzu.testing;
fn main(){
    var caught=false;
    try {rinzu.testing.assertTrue(false,"expected condition");}
    catch [problem:rinzu.testing.AssertionFailed] {
        if problem.message!="expected condition" {panic("lost assertion message");}
        caught=true;
    }
    catch unexpected {panic("unexpected assertion Error");}
    if !caught {panic("missing AssertionFailed");}
}
"#,
    );
    let report = fixture.report_at("tests", &[], true);
    assert_eq!(report["summary"]["passed"], 1);
    assert_eq!(report["caseSummary"]["selected"], 0);
}

#[test]
fn source_sdk_document_imports_its_own_public_api() {
    let fixture = Fixture::new("fn main() {}\n");
    fixture.dependency("testing");
    std::fs::write(
        fixture.0.join("testing/OWN_API.md"),
        "```rinzu-test\nimport rinzu.testing;\nfn main(){try{rinzu.testing.assertEqual<Int>(20+22,42,\"own API\");}catch errors{panic(\"own API failed\");}}\n```\n",
    )
    .unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("test")
        .arg(fixture.0.join("testing"))
        .args(["--doc", "OWN_API.md", "--format", "json"])
        .env("RINZU_CACHE_DIR", fixture.0.join("user-cache"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["summary"]["passed"], 1);
    assert_eq!(report["cases"][0]["settlement"], "confirmed");
}

#[test]
fn native_sdk_test_file_uses_its_own_checked_binding() {
    let fixture = Fixture::native_sdk();
    fixture.file(
        "tests/answer.test.rinzu",
        "import test_native; fn main(){if test_native.answer()!=CInt(42){let failed=CInt8(300);}}",
    );
    let report = fixture.report_at("tests", &[], true);
    assert_eq!(report["summary"]["passed"], 1);
    assert_eq!(report["entries"][0]["worker"]["engine"], "compiled-jit");
    assert_eq!(report["entries"][0]["settlement"], "confirmed");
    assert!(!fixture.0.join(".rinzu/generated").exists());
}

#[test]
fn binary_stdout_stderr_are_drained_bounded_and_never_a_result_protocol() {
    let fixture = Fixture::native_sdk();
    for name in ["a", "b", "c"] {
        fixture.file(
            &format!("tests/{name}.test.rinzu"),
            "import test_native; fn main(){test_native.burst();}",
        );
    }
    let report = fixture.report_at("tests", &[], true);
    assert_eq!(report["summary"]["passed"], 3);
    let cases = report["entries"].as_array().unwrap();
    assert_eq!(stdout(&cases[0]).len(), 1024 * 1024);
    assert!(stdout(&cases[0]).iter().all(|b| *b == 0xa5));
    assert_eq!(cases[0]["stdoutTotal"], 145 * 8192);
    assert_eq!(cases[0]["stdoutTruncated"], true);
    assert_eq!(cases[0]["stderrTruncated"], true);
    let retained: usize = cases
        .iter()
        .map(|c| {
            c["stdoutBytes"].as_array().unwrap().len() + c["stderrBytes"].as_array().unwrap().len()
        })
        .sum();
    assert!(retained <= 4 * 1024 * 1024);
    assert!(stdout(&cases[2]).is_empty());
}

#[test]
fn separate_files_physically_close_resources_after_failure_and_cancellation() {
    let fixture = Fixture::new("fn main() {}");
    fixture.file("tests/support.rinzu", r#"
import rinzu.io; import rinzu.process; import rinzu.stdio;
fn root()->String {return rinzu.process.setting("RINZU_TEST_TMP","missing");}
async fn close(file:rinzu.io.File) {let status=await rinzu.io.close(file);rinzu.stdio.writeLine("CLOSED");}
export close; export root;
"#);
    fixture.file("tests/a.test.rinzu", r#"
import "./support.rinzu" as support;
import rinzu.io; import rinzu.stdio;
async fn main(){rinzu.stdio.writeLine(support.root());let file=await rinzu.io.openWrite(f"{support.root()}/fixture");await support.close(file);}
"#);
    fixture.file("tests/b.test.rinzu", r#"
import "./support.rinzu" as support;
import rinzu.io; import rinzu.stdio;
async fn main(){
    rinzu.stdio.writeLine(support.root());
    var found:Option<rinzu.io.File> = Option<rinzu.io.File>.None();
    try {found=Option<rinzu.io.File>.Some(value:await rinzu.io.openRead(f"{support.root()}/fixture"));}catch errors{}
    switch found{case Option.None:{}case Option.Some(file):{await support.close(file);let bad=CInt8(300);}}
}
"#);
    fixture.file("tests/failure.test.rinzu", r#"
import "./support.rinzu" as support;
import rinzu.io;
async fn main(){let file=await rinzu.io.openWrite(f"{support.root()}/failure");try {let bad=CInt8(300);}finally{await support.close(file);}}
"#);
    fixture.file("tests/cancel.test.rinzu", r#"
import "./support.rinzu" as support;
import rinzu.io; import rinzu.timer;
async fn main(){let file=await rinzu.io.openWrite(f"{support.root()}/cancel");try {let pending=rinzu.timer.sleep(20000);let cancelled=cancelFuture(pending);await pending;}finally{await support.close(file);}}
"#);
    let report = fixture.report_at("tests", &[], false);
    assert_eq!(report["summary"]["passed"], 2, "{report}");
    assert_eq!(report["summary"]["failed"], 2);
    let cases = report["entries"].as_array().unwrap();
    assert!(cases.iter().all(|case| case["settlement"] == "confirmed"));
    let paths: Vec<_> = cases
        .iter()
        .take(2)
        .map(|c| {
            String::from_utf8(stdout(c))
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_ne!(paths[0], paths[1]);
    assert!(paths.iter().all(|path| !Path::new(path).exists()));
    let cancelled = &cases[2];
    assert_eq!(cancelled["status"], "cancelled");
    assert!(stdout(cancelled).windows(6).any(|b| b == b"CLOSED"));
    let failed = &cases[3];
    assert_eq!(failed["status"], "fail");
    assert!(stdout(failed).windows(6).any(|b| b == b"CLOSED"));
}

#[test]
fn timeout_and_interrupt_stop_owned_native_process_groups_and_leave_settlement_unconfirmed() {
    let fixture = Fixture::native_sdk();
    fixture.file(
        "tests/hang.test.rinzu",
        r#"
import test_native;
import rinzu.testing as testing;

fn main() {
    testing.runCase("completed", fn () {});
    testing.runCase("blocked", fn () { test_native.never(); });
}
"#,
    );
    fixture.file("tests/later.test.rinzu", "fn main() {}");
    let marker = fixture.0.join("marker");
    let output = fixture
        .command("tests/hang.test.rinzu")
        .args(["--timeout-ms", "30000", "--format", "json"])
        .env("RINZU_TEST_MARKER", &marker)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let entry = &report["entries"][0];
    assert_eq!(entry["status"], "timeout");
    assert_eq!(entry["settlement"], "unconfirmed");
    assert_eq!(entry["cases"][0]["status"], "pass", "{report:#}");
    assert_eq!(entry["cases"][1]["status"], "timeout");
    let worker: i32 = std::fs::read_to_string(&marker)
        .unwrap_or_else(|error| panic!("native entry marker missing: {error}; report: {report}"))
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only probes the PID reported by this fixture's worker.
    assert_eq!(unsafe { libc::kill(worker, 0) }, -1);
    std::fs::remove_file(&marker).unwrap();
    let child = fixture
        .command("tests")
        .args(["--format", "json"])
        .env("RINZU_TEST_MARKER", &marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut owned = DriverGuard(Some(child));
    let started = Instant::now();
    let worker = loop {
        if let Ok(text) = std::fs::read_to_string(&marker)
            && let Ok(pid) = text.trim().parse::<i32>()
        {
            break pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(45),
            "native entry was not reached"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        // SAFETY: send SIGINT only to the child driver owned by this test.
        unsafe { libc::kill(owned.0.as_ref().unwrap().id() as i32, libc::SIGINT) },
        0
    );
    let output = owned.0.take().unwrap().wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["entries"][0]["status"], "interrupted");
    assert_eq!(report["entries"][0]["settlement"], "unconfirmed");
    assert_eq!(report["entries"][0]["cases"][0]["status"], "pass");
    assert_eq!(report["entries"][0]["cases"][1]["status"], "interrupted");
    assert_eq!(report["entries"][1]["status"], "not-run");
    // SAFETY: signal 0 only probes this fixture's already-waited worker.
    assert_eq!(unsafe { libc::kill(worker, 0) }, -1);
}

struct DriverGuard(Option<std::process::Child>);
impl Drop for DriverGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            // SAFETY: this guard owns the child driver and waits for it below.
            unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            }
            let _ = child.wait();
        }
    }
}
