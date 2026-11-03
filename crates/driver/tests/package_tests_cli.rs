#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rinzu-unit-cli-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("src")).unwrap();
        std::fs::write(
            path.join("rinzu.toml"),
            "[package]\nname=\"unit_app\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        std::fs::write(path.join("src/main.rinzu"), source).unwrap();
        Self(path)
    }
    fn cli(&self, flags: &[&str]) -> Output {
        Command::new(frozen_cli::path())
            .arg("test")
            .arg(&self.0)
            .args(flags)
            .env("RINZU_CACHE_DIR", self.0.join("user-cache"))
            .output()
            .unwrap()
    }
    fn report(&self, flags: &[&str], success: bool) -> Value {
        let mut flags = flags.to_vec();
        flags.extend(["--format", "json"]);
        let output = self.cli(&flags);
        let report: Option<Value> = serde_json::from_slice(&output.stdout).ok();
        let failures = report.as_ref().map(|report| {
            report["cases"].as_array().map(|cases| {
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
    fn native_sdk(source: &str) -> Self {
        let fixture = Self::new(source);
        std::fs::write(fixture.0.join("rinzu.toml"),"[package]\nname=\"unit_native\"\nkind=\"sdk\"\nlanguage=\"0.5\"\nentry=\"src/main.rinzu\"\n[native]\nsources=[\"native/shim.c\"]\nheaders=[\"native/shim.h\"]\n").unwrap();
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
fn legacy_main_and_private_unit_list_filter_json_and_artifact_preservation() {
    let fixture = Fixture::new(
        "import rinzu.stdio; fn main(){try{rinzu.stdio.writeLine(\"legacy\");}catch errors{panic(\"failed\");}} fn test_a(){rinzu.stdio.writeLine(\"RINZU_RESULT=spoof\");} async fn test_b(){}\n",
    );
    std::fs::write(fixture.0.join("src/math.rinzu"),"fn identity<T>(x:T)->T{return x;} fn test_generic(){let value=identity<Int>(42);let add=fn(x:Int)->Int{return x+1;};if add(value)!=43 {let bad=CInt8(300);}}\n").unwrap();
    let original = std::fs::read(fixture.0.join("src/main.rinzu")).unwrap();
    std::fs::create_dir(fixture.0.join("build")).unwrap();
    std::fs::write(fixture.0.join("build/keep"), b"frozen output").unwrap();
    std::fs::create_dir_all(fixture.0.join(".rinzu/generated/source-packages-v2")).unwrap();
    std::fs::write(
        fixture.0.join(".rinzu/generated/source-packages-v2/keep"),
        b"frozen sidecar",
    )
    .unwrap();
    let legacy = fixture.cli(&[]);
    assert!(legacy.status.success());
    assert_eq!(legacy.stdout, b"legacy\n");
    let listed = fixture.report(&["--unit", "--list"], true);
    assert_eq!(listed["cases"].as_array().unwrap().len(), 3);
    assert_eq!(listed["summary"]["passed"], 0);
    let all = fixture.report(&["--unit"], true);
    assert_eq!(all["summary"]["passed"], 3);
    assert_eq!(stdout(&all["cases"][0]), b"RINZU_RESULT=spoof\n");
    assert!(
        all["cases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|case| case["settlement"] == "confirmed")
    );
    let selected = fixture.report(&["--unit", "--filter", "test_generic"], true);
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
fn discovery_errors_empty_filters_and_options_are_non_success() {
    let fixture = Fixture::new("fn test_bad(x:Int){}\n");
    let invalid = fixture.report(&["--unit", "--filter", "not-selected"], false);
    assert!(
        invalid["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("signature")
    );
    std::fs::write(fixture.0.join("src/main.rinzu"), "fn test_ok(){}\n").unwrap();
    let empty = fixture.report(&["--unit", "--filter", "missing"], false);
    assert_eq!(empty["summary"]["passed"], 0);
    for flags in [
        &["--list"][..],
        &["--unit", "--unit"],
        &["--unit", "--filter", ""],
        &["--unit", "--timeout-ms", "0"],
        &["--unit", "--format", "xml"],
        &["--unit", "--resource-report", "report.json"],
        &["--unit", "--unknown"],
    ] {
        assert!(!fixture.cli(flags).status.success(), "{flags:?}");
    }
    std::fs::write(
        fixture.0.join("src/unused.rinzu"),
        "fn test_unused(){let bad:Int=\"wrong\";}\n",
    )
    .unwrap();
    assert!(
        !fixture.report(&["--unit", "--filter", "test_ok"], false)["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
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
fn source_assertions_keep_primary_cleanup_and_expected_values() {
    let fixture = Fixture::new(
        "import rinzu.testing; fn test_assert(){try {rinzu.testing.assertEqual<Int>(3,4,\"sum\");}finally{let cleanup=CInt8(300);}}\n",
    );
    fixture.dependency("testing");
    let report = fixture.report(&["--unit"], false);
    let case = &report["cases"][0];
    assert_eq!(case["status"], "fail", "{case}");
    assert_eq!(case["settlement"], "confirmed");
    let text = String::from_utf8(stdout(case)).unwrap();
    assert!(text.contains("actual:\n3\nexpected:\n4\n"));
    assert_eq!(case["worker"]["failure"]["primary"]["cause"], "Error(User)");
    assert_eq!(
        case["worker"]["failure"]["cleanup"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let worker = &case["worker"];
    let file = worker["failure"]["primary"]["source"]["file"]
        .as_u64()
        .unwrap()
        .to_string();
    let label = worker["sourceFiles"][&file].as_str().unwrap();
    assert!(
        worker["sourcePaths"][label]
            .as_str()
            .unwrap()
            .ends_with("testing/src/testing.rinzu")
    );
    assert_eq!(
        worker["resourceReport"][0]["sources"],
        worker["sourceFiles"]
    );
}

#[test]
fn assertion_failures_have_their_own_catchable_error_and_message() {
    let fixture = Fixture::new(
        r#"import rinzu.testing;
fn test_assertion_type(){
    var caught=false;
    try {rinzu.testing.assertTrue(false,"expected condition");}
    catch [problem:rinzu.testing.AssertionFailed] {
        if problem.message!="expected condition" {panic("lost assertion message");}
        caught=true;
    }
    catch unexpected {panic("unexpected assertion Error");}
    if !caught {panic("missing AssertionFailed");}
}
fn main(){}"#,
    );
    fixture.dependency("testing");
    let report = fixture.report(&["--unit"], true);
    assert_eq!(report["summary"]["passed"], 1);
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
fn native_sdk_private_units_use_own_header_and_checked_binding() {
    let fixture = Fixture::native_sdk(
        "native {fn raw()->CInt symbol \"answer\";} fn test_private(){if raw()!=CInt(42){let failed=CInt8(300);}}\n",
    );
    let report = fixture.report(&["--unit"], true);
    assert_eq!(report["summary"]["passed"], 1);
    assert_eq!(report["cases"][0]["worker"]["engine"], "compiled-jit");
    assert_eq!(report["cases"][0]["settlement"], "confirmed");
    assert!(!fixture.0.join(".rinzu/generated").exists());
}

#[test]
fn binary_stdout_stderr_are_drained_bounded_and_never_a_result_protocol() {
    let fixture = Fixture::native_sdk(
        "native {fn rawBurst() symbol \"burst\";} fn test_a(){rawBurst();} fn test_b(){rawBurst();} fn test_c(){rawBurst();}\n",
    );
    let report = fixture.report(&["--unit"], true);
    assert_eq!(report["summary"]["passed"], 3);
    let cases = report["cases"].as_array().unwrap();
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
fn failure_cancel_and_isolated_fixture_resources_are_physically_closed() {
    let fixture = Fixture::new(
        r#"
import rinzu.io; import rinzu.timer; import rinzu.process; import rinzu.stdio;
fn root()->String {return rinzu.process.setting("RINZU_TEST_TMP","missing");}
async fn close(file:rinzu.io.File) {let status=await rinzu.io.close(file);rinzu.stdio.writeLine("CLOSED");}
async fn test_a_fixture(){rinzu.stdio.writeLine(root());let file=await rinzu.io.openWrite(f"{root()}/fixture");await close(file);}
async fn test_b_isolated(){
    rinzu.stdio.writeLine(root());
    var found:Option<rinzu.io.File> = Option<rinzu.io.File>.None();
    try {found=Option<rinzu.io.File>.Some(value:await rinzu.io.openRead(f"{root()}/fixture"));}catch errors{}
    switch found{case Option.None:{}case Option.Some(file):{await close(file);let bad=CInt8(300);}}
}
async fn test_failure(){let file=await rinzu.io.openWrite(f"{root()}/failure");try {let bad=CInt8(300);}finally{await close(file);}}
async fn test_cancel(){let file=await rinzu.io.openWrite(f"{root()}/cancel");try {let pending=rinzu.timer.sleep(20000);let cancelled=cancelFuture(pending);await pending;}finally{await close(file);}}

"#,
    );
    fixture.dependency("io");
    fixture.dependency("process");
    let report = fixture.report(&["--unit"], false);
    assert_eq!(report["summary"]["passed"], 2, "{report}");
    assert_eq!(report["summary"]["failed"], 2);
    let cases = report["cases"].as_array().unwrap();
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
    let cancelled = cases
        .iter()
        .find(|c| c["identity"].as_str().unwrap().ends_with("test_cancel"))
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert!(stdout(cancelled).windows(6).any(|b| b == b"CLOSED"));
    let failed = cases
        .iter()
        .find(|c| c["identity"].as_str().unwrap().ends_with("test_failure"))
        .unwrap();
    assert_eq!(failed["status"], "fail");
    assert!(stdout(failed).windows(6).any(|b| b == b"CLOSED"));
}

#[test]
fn timeout_and_interrupt_stop_owned_native_process_groups_and_leave_settlement_unconfirmed() {
    let fixture = Fixture::native_sdk(
        "native {fn rawNever() symbol \"never\";} fn test_hang(){rawNever();} fn test_later(){}\n",
    );
    let marker = fixture.0.join("marker");
    let output = Command::new(frozen_cli::path())
        .arg("test")
        .arg(&fixture.0)
        .args([
            "--unit",
            "--filter",
            "test_hang",
            "--timeout-ms",
            "30000",
            "--format",
            "json",
        ])
        .env("RINZU_CACHE_DIR", fixture.0.join("user-cache"))
        .env("RINZU_TEST_MARKER", &marker)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["cases"][0]["status"], "timeout");
    assert_eq!(report["cases"][0]["settlement"], "unconfirmed");
    let worker: i32 = std::fs::read_to_string(&marker)
        .unwrap_or_else(|error| panic!("native entry marker missing: {error}; report: {report}"))
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only probes the PID reported by this fixture's worker.
    assert_eq!(unsafe { libc::kill(worker, 0) }, -1);
    std::fs::remove_file(&marker).unwrap();
    let child = Command::new(frozen_cli::path())
        .arg("test")
        .arg(&fixture.0)
        .args(["--unit", "--format", "json"])
        .env("RINZU_CACHE_DIR", fixture.0.join("user-cache"))
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
    assert_eq!(report["cases"][0]["status"], "interrupted");
    assert_eq!(report["cases"][0]["settlement"], "unconfirmed");
    assert_eq!(report["cases"][1]["status"], "not-run");
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
