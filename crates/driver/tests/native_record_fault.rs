#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
#[allow(dead_code)]
mod platform;

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "fixtures/native_record_sdk/setup.rs"]
mod records;

fn cli(app: &Path, command: &str) -> Output {
    platform::bounded_tool(Command::new(frozen_cli::path()).arg(command).arg(app))
}
fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn unreadable_record_result_is_a_native_fault_on_source_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-record-read-fault-{}", std::process::id()));
    records::create(&root);
    let app = root.join("app");
    for entry in ["fn main()", "async fn main()"] {
        std::fs::write(app.join("src/main.rinzu"), format!("import records; {entry} {{try {{let failed=records.fatal();}} catch copyError {{panic(\"incorrectlyRecovered\");}} finally {{let resumed=7;}}}}" )).unwrap();
        let source = cli(&app, "run");
        assert!(
            matches!(source.status.signal(), Some(10 | 11)),
            "{}: {}",
            source.status,
            String::from_utf8_lossy(&source.stderr)
        );
        success(&cli(&app, "build"));
        let aot = Command::new(app.join("build/record_app")).output().unwrap();
        assert!(
            matches!(aot.status.signal(), Some(10 | 11)),
            "{}: {}",
            aot.status,
            String::from_utf8_lossy(&aot.stderr)
        );
        assert!(!String::from_utf8_lossy(&aot.stderr).contains("RINZU_RESULT="));
    }
    std::fs::remove_dir_all(root).unwrap();
}

fn replace(source: String, needle: &str, replacement: &str) -> String {
    assert_eq!(source.matches(needle).count(), 1, "{needle}");
    source.replace(needle, replacement)
}

fn fixture(root: &Path) {
    records::create(root);
    let api = root.join("records/src/api.rinzu");
    let source = std::fs::read_to_string(&api).unwrap();
    std::fs::write(api, format!("{source}\nfn faultInput()->CInt {{return check(original(),false);}}\nexport faultInput;\n")).unwrap();
    let path = root.join("records/api.c");
    let source = std::fs::read_to_string(&path).unwrap();
    let mut source = format!(
        r#"#include <stdlib.h>
#include <stdio.h>
#include <fcntl.h>
#include <unistd.h>
static void record_test_entry(void) {{
 const char *path=getenv("RINZU_TEST_RECORD_ENTRY");
 if (path) {{FILE *file=fopen(path,"ab");if(!file) _Exit(90);fputc('E',file);if(fclose(file)) _Exit(90);}}
}}
{source}"#
    );
    for needle in [
        "const struct Packet *record_result(int32_t kind) {",
        "int32_t record_check(const struct Packet *value, _Bool nan) {",
    ] {
        source = replace(source, needle, &format!("{needle} record_test_entry();"));
    }
    std::fs::write(path, source).unwrap();
}

fn instrumented(app: &Path, sanitized: bool) -> PathBuf {
    let mut runtime = std::fs::read_to_string(app.join("build/record_app.runtime.c")).unwrap();
    for (variable, call) in [
        (
            "RINZU_TEST_PREPARE_BUDGET",
            "int status = rinzu_native_record_prepare(",
        ),
        (
            "RINZU_TEST_RECEIVE_BUDGET",
            "int status = rinzu_native_receive_record_prepared(",
        ),
    ] {
        assert_eq!(runtime.matches(call).count(), 1, "{call}");
        let start = runtime.find(call).unwrap();
        let end = start + runtime[start..].find(';').unwrap() + 1;
        let statement = &runtime[start..end];
        let instrumented =
            format!("rinzu_test_arm(\"{variable}\"); {statement} rinzu_test_budget=-1;");
        runtime.replace_range(start..end, &instrumented);
    }
    runtime = replace(
        runtime,
        "atomic_fetch_add(&rinzu_native_evidence.returned, 1);",
        "atomic_fetch_add(&rinzu_native_evidence.returned, 1); rinzu_test_marker('R');",
    );
    let runtime = format!(
        r#"#include <stdlib.h>
#include <stdio.h>
static _Thread_local long rinzu_test_budget=-1;
static void rinzu_test_marker(int marker) {{
  const char *path=getenv("RINZU_TEST_RECORD_ENTRY");
  if (path) {{ FILE *file=fopen(path,"ab"); if (!file) _Exit(90); fputc(marker,file); if (fclose(file)) _Exit(90); }}
}}
static void rinzu_test_arm(const char *name) {{
  const char *value=getenv(name); rinzu_test_budget=value ? strtol(value,NULL,10) : -1;
}}
static int rinzu_test_allowed(void) {{
  if (!rinzu_test_budget) return 0;
  if (rinzu_test_budget>0) --rinzu_test_budget;
  return 1;
}}
static void *rinzu_test_malloc(size_t size) {{return rinzu_test_allowed() ? malloc(size) : NULL;}}
static void *rinzu_test_calloc(size_t count,size_t size) {{return rinzu_test_allowed() ? calloc(count,size) : NULL;}}
static void *rinzu_test_realloc(void *pointer,size_t size) {{return rinzu_test_allowed() ? realloc(pointer,size) : NULL;}}
static int rinzu_test_aligned(void **pointer,size_t alignment,size_t size) {{
  if (!rinzu_test_allowed()) {{*pointer=NULL;return 12;}} return posix_memalign(pointer,alignment,size);
}}
#define malloc rinzu_test_malloc
#define calloc rinzu_test_calloc
#define realloc rinzu_test_realloc
#define posix_memalign rinzu_test_aligned
{runtime}"#
    );
    let suffix = if sanitized { "sanitized" } else { "fault" };
    let source = app.join(format!("build/record_app-{suffix}.c"));
    let binary = app.join(format!("build/record_app-{suffix}"));
    std::fs::write(&source, runtime).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut clang = Command::new("clang");
    clang
        .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
        .arg(app.join("build/record_app.o"))
        .arg(&source)
        .arg(manifest.join("../runtime/src/callback_entry.c"))
        .arg(manifest.join("../runtime/src/callback_template.S"))
        .arg("-I")
        .arg(manifest.join("../../include"));
    if sanitized {
        clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
    }
    success(&clang.arg("-o").arg(&binary).output().unwrap());
    binary
}

fn execute(binary: &Path, marker: &Path, variable: &str, budget: usize) -> Output {
    let _ = std::fs::remove_file(marker);
    Command::new(binary)
        .env(variable, budget.to_string())
        .env("RINZU_TEST_RECORD_ENTRY", marker)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("UBSAN_OPTIONS", "halt_on_error=1")
        .output()
        .unwrap()
}

#[test]
fn record_allocator_faults_never_enter_or_resume_and_receipt_errors_remain_recoverable() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-record-allocator-fault-{}",
        std::process::id()
    ));
    fixture(&root);
    let app = root.join("app");
    let marker = root.join("physical-entry-return");
    for (expression, variable, returned) in [
        ("records.faultInput()", "RINZU_TEST_PREPARE_BUDGET", false),
        ("records.failure(1)", "RINZU_TEST_RECEIVE_BUDGET", true),
    ] {
        std::fs::write(app.join("src/main.rinzu"),format!("import records;async fn main(){{try{{let value={expression};}}catch errors{{panic(\"resumedAfterOom\");}}}}")).unwrap();
        success(&cli(&app, "build"));
        for sanitized in [false, true] {
            let binary = instrumented(&app, sanitized);
            let mut failed = 0;
            let mut completed = false;
            for budget in 0..20 {
                let run = execute(&binary, &marker, variable, budget);
                if !run.status.success() {
                    assert!(
                        run.status.code() == Some(71),
                        "{variable}={budget}: {run:?}"
                    );
                    if variable == "RINZU_TEST_PREPARE_BUDGET" && budget == 0 {
                        assert_eq!(run.status.code(), Some(71));
                    }
                    assert_eq!(
                        std::fs::read(&marker).unwrap_or_default(),
                        if returned { b"ER".to_vec() } else { vec![] },
                        "{variable}={budget}"
                    );
                    assert!(!String::from_utf8_lossy(&run.stderr).contains("RINZU_RESULT="));
                    failed += 1;
                    continue;
                }
                assert_eq!(std::fs::read(&marker).unwrap(), b"ER");
                let report = String::from_utf8_lossy(&run.stderr);
                assert!(
                    report.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,"),
                    "{report}"
                );
                assert!(report.contains("\"resources\":\"baseline\""), "{report}");
                completed = true;
                break;
            }
            assert!(failed > 0 && completed, "{variable}");
        }
    }
    for (kind, cause, budget) in [(0, 8, 0), (2, 5, 1)] {
        std::fs::write(app.join("src/main.rinzu"),format!("import records;async fn main(){{var caught=0;try{{let invalid=records.failure({kind});}}catch [error:Error,...rest]{{caught=caught+1;}}if caught!=1{{panic(\"missing receipt error\");}}}}")).unwrap();
        success(&cli(&app, "build"));
        for sanitized in [false, true] {
            let binary = instrumented(&app, sanitized);
            let run = execute(&binary, &marker, "RINZU_TEST_RECEIVE_BUDGET", budget);
            success(&run);
            let report = String::from_utf8_lossy(&run.stderr);
            assert!(
                report.contains(&format!("RINZU_CAUGHT_ERRORS={cause}@")),
                "{report}"
            );
            assert!(
                report.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,"),
                "{report}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_native_record_writeback_exits_86_before_catch_or_finally() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-record-invalid-writeback-{}",
        std::process::id()
    ));
    fixture(&root);
    let source = root.join("records/api.c");
    let native = std::fs::read_to_string(&source).unwrap();
    std::fs::write(
        source,
        replace(
            native,
            "return memcmp(&expected, value, sizeof(expected)) == 0;",
            r#"int valid = memcmp(&expected, value, sizeof(expected)) == 0;
    const char *full=getenv("RINZU_TEST_FATAL_STDERR_FULL");
    if (full && *full == '1') {
      int pipes[2]; if (pipe(pipes)) _Exit(90);
      int flags=fcntl(pipes[1],F_GETFL);
      if (flags<0 || fcntl(pipes[1],F_SETFL,flags|O_NONBLOCK)) _Exit(90);
      unsigned char bytes[4096]={0};
      while(write(pipes[1],bytes,sizeof(bytes))>0) {}
      if(fcntl(pipes[1],F_SETFL,flags) || dup2(pipes[1],2)!=2) _Exit(90);
    }
    unsigned char invalid = 2;
    memcpy((unsigned char *)value + offsetof(struct Packet, k), &invalid, 1);
    return valid;"#,
        ),
    )
    .unwrap();
    let app = root.join("app");
    for entry in ["fn main()", "async fn main()"] {
        std::fs::write(app.join("src/main.rinzu"),format!("import records;import rinzu.stdio;{entry}{{try{{let value=records.faultInput();}}catch errors{{panic(\"catch resumed\");}}finally{{panic(\"finally resumed\");}}}}")).unwrap();
        for full in [false, true] {
            let jit = platform::bounded_tool(
                Command::new(frozen_cli::path())
                    .arg("run")
                    .arg(&app)
                    .env("RINZU_TEST_FATAL_STDERR_FULL", if full { "1" } else { "" }),
            );
            assert_eq!(jit.status.code(), Some(86), "{jit:?}");
            assert!(jit.stdout.is_empty());
        }
        success(&cli(&app, "build"));
        for binary in [app.join("build/record_app"), instrumented(&app, true)] {
            for full in [false, true] {
                let mut command = Command::new(&binary);
                if full {
                    command.env("RINZU_TEST_FATAL_STDERR_FULL", "1");
                }
                let run = platform::bounded(
                    command
                        .env("ASAN_OPTIONS", "detect_leaks=0")
                        .env("UBSAN_OPTIONS", "halt_on_error=1"),
                );
                assert_eq!(run.status.code(), Some(86), "{run:?}");
                assert!(run.stdout.is_empty());
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
