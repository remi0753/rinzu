#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_callback_sdk/faults.rs"]
mod faults;
#[path = "fixtures/native_callback_sdk/setup.rs"]
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
    let mut source = std::fs::read_to_string(app.join("build/callback_app.runtime.c")).unwrap();
    source = replace(
        source,
        "    rinzu_baseline_leave_dispatch(context);",
        "    if(context->pending.current.primary && getenv(\"RINZU_TEST_CALLBACK_TEARDOWN\")) {rinzu_test_budget=0;rinzu_test_callback_body_teardowns++;} rinzu_baseline_leave_dispatch(context);",
    );
    source = replace(
        source,
        "const RinzuNativeCallbackPlan *plan = callback->plan;",
        r#"
const RinzuNativeCallbackPlan *plan=callback->plan;
rinzu_test_budget=-1;
rinzu_test_arm("RINZU_TEST_CALLBACK_ACTIVITY");
"#,
    );
    source = replace(
        source,
        "void *previous_scope = rinzu_aot_callback_enter_scope(context, callback->parent_scope);",
        r#"
void *previous_scope=rinzu_aot_callback_enter_scope(context,callback->parent_scope);
if (!rinzu_test_callback_armed) {rinzu_test_callback_armed=1;rinzu_test_arm("RINZU_TEST_CALLBACK_PREPARE");rinzu_test_callback_initial=rinzu_test_budget;}
"#,
    );
    source = replace(
        source,
        "uint64_t result = 0;\n  if (!input_error) {",
        r#"
if (rinzu_test_callback_initial>=0) {
  if(input_error) _Exit(90);
  printf("RINZU_CALLBACK_PREPARE_ALLOCATIONS=%ld\n",rinzu_test_callback_initial-rinzu_test_budget);
  rinzu_test_callback_initial=-1;rinzu_test_budget=-1;
}
if(input_error && getenv("RINZU_TEST_CALLBACK_TEARDOWN")) {rinzu_test_budget=0;rinzu_test_callback_teardowns++;}
uint64_t result=0;
  if (!input_error) {
"#,
    );
    source = replace(
        source,
        "free(call);\n}\n#ifdef RINZU_AOT_HAS_SUPPLANT",
        "free(call); rinzu_test_budget=-1;\n}\n#ifdef RINZU_AOT_HAS_SUPPLANT",
    );
    source = replace(
        source,
        "  rinzu_aot_source_native_startup();",
        "  rinzu_aot_source_native_startup(); if(pthread_once(&rinzu_program_schema_once,rinzu_init_program_schema)) _Exit(90);rinzu_test_baseline_count=rinzu_test_count;rinzu_test_baseline_bytes=rinzu_test_bytes;",
    );
    source = replace(
        source,
        "return exit_code;",
        r#"
rinzu_userdata_process_stats_t process=rinzu_userdata_process_stats();
if(rinzu_test_count!=rinzu_test_baseline_count+process.tokens || rinzu_test_bytes!=rinzu_test_baseline_bytes+process.token_bytes) _Exit(90);
printf("RINZU_CALLBACK_TEARDOWNS=%zu\n",rinzu_test_callback_teardowns);
printf("RINZU_CALLBACK_BODY_TEARDOWNS=%zu\n",rinzu_test_callback_body_teardowns);
puts("PASS callback allocation baseline restored without token metadata");return exit_code;
"#,
    );
    let prelude = include_str!("fixtures/native_userdata_sdk/allocator.h");
    let prelude = format!(
        "{prelude}\nstatic int rinzu_test_callback_armed;static long rinzu_test_callback_initial=-1;static size_t rinzu_test_callback_teardowns,rinzu_test_callback_body_teardowns;\n"
    );
    let path = app.join(if sanitized {
        "build/callback-fault-sanitized.c"
    } else {
        "build/callback-fault.c"
    });
    let binary = path.with_extension("");
    std::fs::write(&path, format!("{prelude}\n{source}")).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("clang");
    command
        .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
        .arg(app.join("build/callback_app.o"))
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
fn probe(binary: &Path, budget: Option<usize>, teardown: bool) -> Output {
    let mut command = Command::new(binary);
    command
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("RINZU_TEST_CALLBACK_FAULT", "allocation");
    if let Some(budget) = budget {
        command.env("RINZU_TEST_CALLBACK_PREPARE", budget.to_string());
    }
    if teardown {
        command.env("RINZU_TEST_CALLBACK_TEARDOWN", "1");
    }
    command.output().unwrap()
}
fn number(stdout: &str, prefix: &str) -> usize {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("missing {prefix}: {stdout}"))
        .parse()
        .unwrap()
}

#[test]
fn callback_actual_parameter_preparation_oom_is_fatal_and_partial_fallback_teardown_needs_no_allocation()
 {
    let root = std::env::temp_dir().join(format!(
        "rinzu-callback-parameter-fault-{}",
        std::process::id()
    ));
    fixture::create(&root);
    std::fs::write(root.join("callbacks/api.c"), faults::source()).unwrap();
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        "import callbacks;fn main(){let returned=callbacks.exercise();}",
    )
    .unwrap();
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    let mut allocations = None;
    for sanitized in [false, true] {
        let binary = instrument(&app, sanitized);
        let complete = probe(&binary, Some(10000), false);
        success(&complete);
        let stdout = String::from_utf8_lossy(&complete.stderr);
        let count = number(&stdout, "RINZU_CALLBACK_PREPARE_ALLOCATIONS=");
        assert!(count > 0 && count < 10000, "{stdout}");
        assert_eq!(*allocations.get_or_insert(count), count);
        let witness = String::from_utf8_lossy(&complete.stdout);
        assert!(
            witness.contains("PASS callback allocation baseline"),
            "{witness}"
        );
        // The callback diagnostic record is prepared before argument allocation
        // and has the native protocol's separate immediate-fatal allocator.
        let activity = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .env("RINZU_TEST_CALLBACK_FAULT", "allocation")
            .env("RINZU_TEST_CALLBACK_ACTIVITY", "0")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&activity.stderr);
        let stdout = String::from_utf8_lossy(&activity.stderr);
        assert_eq!(activity.status.code(), Some(71), "{stderr}");
        assert!(
            stderr.contains("CALLBACK_NATIVE_ENTER") && !stderr.contains("CALLBACK_BODY_ENTER")
        );
        assert!(!stdout.contains("RINZU_RESULT=") && !stdout.contains("RINZU_CAUGHT_ERRORS="));
        assert!(!stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"));
        for budget in 0..count {
            let output = probe(&binary, Some(budget), false);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(71),
                "sanitized={sanitized},budget={budget}: {stderr}"
            );
            assert!(
                stderr.contains("CALLBACK_NATIVE_ENTER") && !stderr.contains("CALLBACK_BODY_ENTER"),
                "{stderr}"
            );
            assert!(
                !stdout.contains("RINZU_RESULT=") && !stdout.contains("RINZU_CAUGHT_ERRORS="),
                "{stdout}"
            );
            assert!(
                !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
                "{stderr}"
            );
        }
        let partial = probe(&binary, None, true);
        success(&partial);
        let stdout = String::from_utf8_lossy(&partial.stderr);
        let witness = String::from_utf8_lossy(&partial.stdout);
        assert_eq!(number(&stdout, "RINZU_CALLBACK_TEARDOWNS="), 25);
        assert_eq!(number(&stdout, "RINZU_CALLBACK_BODY_TEARDOWNS="), 8);
        assert!(
            stdout.contains("RINZU_NATIVE_CALLS=79,79,79,79,79,79,76,884,0,0,0")
                && stdout.contains("RINZU_NATIVE_USERDATA=0,0,0,0,72,72")
                && stdout.contains("RINZU_USERDATA_PROCESS=0,0,0")
                && witness.contains("PASS callback allocation baseline"),
            "{stdout}\n{witness}"
        );
    }
    eprintln!(
        "verified {} callback preparation allocation positions",
        allocations.unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}
