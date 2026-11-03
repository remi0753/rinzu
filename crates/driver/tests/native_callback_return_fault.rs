#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_callback_return_sdk/faults.rs"]
mod faults;
#[path = "fixtures/native_callback_return_sdk/setup.rs"]
mod fixture;

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}
fn replace(source: String, needle: &str, replacement: &str) -> String {
    assert_eq!(source.matches(needle).count(), 1, "{needle}");
    source.replace(needle, replacement)
}
fn instrument(app: &Path, sanitized: bool) -> PathBuf {
    let mut source = std::fs::read_to_string(app.join("build/return_app.runtime.c")).unwrap();
    source = replace(
        source,
        "#include \"rinzu/callback_return.h\"",
        r#"
#include "rinzu/callback_return.h"
static const unsigned char *rinzu_test_return_copy(const unsigned char *input,size_t prefix,size_t length,unsigned string,unsigned fallback) {
  const char *kind=getenv("RINZU_TEST_RETURN_KIND");
  const char *phase=getenv("RINZU_TEST_RETURN_PHASE");
  int selected=kind && phase && !rinzu_test_return_armed && fallback==(unsigned)!strcmp(phase,"fallback") &&
    ((!strcmp(kind,"string") && string && length) || (!strcmp(kind,"bytes") && !string && length==4) ||
     (!strcmp(kind,"empty") && string && !length) || (!strcmp(kind,"zero") && !string && !length));
  if(selected){rinzu_test_return_armed=1;rinzu_test_arm("RINZU_TEST_RETURN_BUDGET");}
  long initial=rinzu_test_budget;
  const unsigned char *result=rinzu_callback_return_buffer_copy(input,prefix,length,string,fallback);
  if(selected){printf("RINZU_RETURN_ALLOCATION_STEPS=%ld\n",initial-rinzu_test_budget);rinzu_test_budget=-1;}
  return result;
}
#define rinzu_callback_return_buffer_copy rinzu_test_return_copy
"#,
    );
    source = replace(
        source,
        "const RinzuNativeCallbackPlan *plan = callback->plan;",
        "const RinzuNativeCallbackPlan *plan=callback->plan;rinzu_test_budget=-1;",
    );
    source = replace(
        source,
        "uint64_t result = 0;\n  if (!input_error) {",
        r#"
if(input_error && getenv("RINZU_TEST_RETURN_TEARDOWN")){rinzu_test_budget=0;rinzu_test_return_input_teardowns++;}
uint64_t result=0;
  if (!input_error) {
"#,
    );
    source = replace(
        source,
        "    rinzu_baseline_leave_dispatch(context);",
        "    if(context->pending.current.primary && getenv(\"RINZU_TEST_RETURN_TEARDOWN\")){rinzu_test_budget=0;rinzu_test_return_body_teardowns++;} rinzu_baseline_leave_dispatch(context);",
    );
    source = replace(
        source,
        "free(call->callbacks);\n  free(call->buffers);\n  free(call->words);\n  free(call);\n}\n#ifdef RINZU_AOT_HAS_SUPPLANT",
        "free(call->callbacks); free(call->buffers); free(call->words); free(call);rinzu_test_budget=-1;\n}\n#ifdef RINZU_AOT_HAS_SUPPLANT",
    );
    source = replace(
        source,
        "  rinzu_aot_source_native_startup();",
        "  rinzu_aot_source_native_startup();if(pthread_once(&rinzu_program_schema_once,rinzu_init_program_schema))_Exit(90);rinzu_test_baseline_count=rinzu_test_count;rinzu_test_baseline_bytes=rinzu_test_bytes;",
    );
    source = replace(
        source,
        "return exit_code;",
        r#"
rinzu_callback_return_stats_t process=rinzu_callback_return_buffer_stats();
size_t retained=process.normal+process.fallback;
if(rinzu_test_count!=rinzu_test_baseline_count+2*retained || rinzu_test_bytes!=rinzu_test_baseline_bytes+process.bytes+process.metadata_bytes)_Exit(90);
printf("RINZU_RETURN_TEARDOWNS=%zu,%zu\n",rinzu_test_return_input_teardowns,rinzu_test_return_body_teardowns);
puts("PASS callback return allocation baseline contains process buffers only");return exit_code;
"#,
    );
    let prelude = include_str!("fixtures/native_userdata_sdk/allocator.h");
    let path = app.join(if sanitized {
        "build/return-fault-sanitized.c"
    } else {
        "build/return-fault.c"
    });
    let binary = path.with_extension("");
    std::fs::write(&path,format!("{prelude}\nstatic int rinzu_test_return_armed;static size_t rinzu_test_return_input_teardowns,rinzu_test_return_body_teardowns;\n{source}")).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("clang");
    command
        .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
        .arg(app.join("build/return_app.o"))
        .arg(path)
        .arg(manifest.join("../runtime/src/callback_entry.c"))
        .arg(manifest.join("../runtime/src/callback_template.S"))
        .arg("-I")
        .arg(manifest.join("../../include"));
    if sanitized {
        command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
    }
    success(&command.arg("-o").arg(&binary).output().unwrap());
    binary
}
fn probe(binary: &Path, kind: &str, phase: &str, budget: usize) -> Output {
    Command::new(binary)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("RINZU_TEST_RETURN_KIND", kind)
        .env("RINZU_TEST_RETURN_PHASE", phase)
        .env("RINZU_TEST_RETURN_BUDGET", budget.to_string())
        .output()
        .unwrap()
}

#[test]
fn actual_callback_return_buffer_and_fallback_oom_is_fatal_and_fallback_teardown_is_allocation_free()
 {
    let root = std::env::temp_dir().join(format!(
        "rinzu-callback-return-fault-{}",
        std::process::id()
    ));
    fixture::create(&root);
    std::fs::write(root.join("returns/api.c"), faults::source()).unwrap();
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        "import returns;fn main(){let returned=returns.exercise();}",
    )
    .unwrap();
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    for sanitized in [false, true] {
        let binary = instrument(&app, sanitized);
        for (kind, marker) in [
            ("string", "STRING"),
            ("bytes", "BYTES"),
            ("empty", "EMPTY"),
            ("zero", "ZERO"),
        ] {
            for phase in ["normal", "fallback"] {
                let complete = probe(&binary, kind, phase, 10000);
                success(&complete);
                let stdout = String::from_utf8_lossy(&complete.stderr);
                let witness = String::from_utf8_lossy(&complete.stdout);
                assert!(
                    stdout.contains("RINZU_RETURN_ALLOCATION_STEPS=2\n")
                        && witness.contains("PASS callback return allocation baseline"),
                    "{stdout}\n{witness}"
                );
                for budget in 0..2 {
                    let output = probe(&binary, kind, phase, budget);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let stdout = String::from_utf8_lossy(&output.stderr);
                    assert_eq!(
                        output.status.code(),
                        Some(71),
                        "{kind}/{phase}/{budget}/{sanitized}: {stderr}"
                    );
                    assert_eq!(
                        stderr.contains(&format!("RETURN_NATIVE_{marker}_ENTER")),
                        phase == "normal",
                        "{stderr}"
                    );
                    if phase == "normal" {
                        assert!(stderr.contains("RETURN_FINALLY\n"));
                    }
                    assert!(
                        !stdout.contains("RINZU_RESULT=")
                            && !stdout.contains("RINZU_CAUGHT_ERRORS="),
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
        let complete = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("RINZU_TEST_RETURN_TEARDOWN", "1")
            .output()
            .unwrap();
        success(&complete);
        let stdout = String::from_utf8_lossy(&complete.stderr);
        let witness = String::from_utf8_lossy(&complete.stdout);
        assert!(
            stdout.contains("RINZU_RETURN_TEARDOWNS=8,28\n")
                && stdout.contains("RINZU_CALLBACK_RETURNS=24,12,266,864")
                && stdout.contains("RINZU_NATIVE_CALLS=70,70,70,70,70,70,56,160,0,0,0")
                && witness.contains("PASS callback return allocation baseline"),
            "{stdout}\n{witness}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
