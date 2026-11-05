#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_data_sdk/setup.rs"]
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

#[test]
fn native_data_cli_compares_values_caught_errors_and_rejects_uncaught_roots() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-native-data-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    success(&cli(&app, "run"));
    success(&cli(&app, "build"));
    let binary = app.join("build/buffer_app");
    let aot = Command::new(&binary).output().unwrap();
    success(&aot);
    assert!(
        String::from_utf8_lossy(&aot.stderr)
            .contains("RINZU_NATIVE_CALLS=166,166,166,166,166,166,288,1152,0,0,0")
    );
    success(&cli(&app, "verify-aot"));
    std::fs::write(app.join("src/main.rinzu"), "import buffers; fn main() { let result = buffers.exercise(b\"A\\xff\", \"日本語\\u{0}🙂\"); if result.caught != 7 { panic(\"wrong\"); } }").unwrap();
    let jit = cli(&app, "run");
    success(&jit);
    assert!(String::from_utf8_lossy(&jit.stderr).contains("baseline JIT"));
    success(&cli(&app, "verify-aot"));
    for main in ["fn main()", "async fn main()"] {
        for (kind, cause, message) in [
            (0, 8, "native value is NULL"),
            (3, 9, "native string is not valid UTF-8"),
        ] {
            std::fs::write(
                app.join("src/main.rinzu"),
                format!("import buffers; {main} {{let failed=buffers.failure({kind});}}"),
            )
            .unwrap();
            let rejected = cli(&app, "check");
            assert!(!rejected.status.success());
            assert!(String::from_utf8_lossy(&rejected.stderr).contains("RZ0413"));
            std::fs::write(app.join("src/main.rinzu"), format!(r#"import rinzu.List;
import buffers; {main} {{
                var caught=false;
                try {{let failed=buffers.failure({kind});}}
                catch [problem:Error,...rest] {{caught=true;if problem.message!="{message}"||rinzu.List.length(rest)!=0{{panic("wrongError");}}}}
                if !caught{{panic("missingError");}}
            }}"#)).unwrap();
            success(&cli(&app, "run"));
            success(&cli(&app, "build"));
            let aot = Command::new(&binary).output().unwrap();
            success(&aot);
            let report = String::from_utf8_lossy(&aot.stderr);
            assert!(
                report.contains(&format!("RINZU_CAUGHT_ERRORS={cause}@")),
                "{report}"
            );
            assert!(report.contains("RINZU_FAILURE=none"), "{report}");
            assert!(
                report.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0"),
                "{report}"
            );
            success(&cli(&app, "verify-aot"));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_data_copy_faults_terminate_before_catch_on_source_jit_and_aot() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-cli-source-data-fault-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let app = root.join("app");
    for main in ["fn main()", "async fn main()"] {
        for kind in 0..=2 {
            std::fs::write(app.join("src/main.rinzu"), format!("import buffers; {main} {{ try {{ let invalid = buffers.fatal({kind}); }} catch copyError {{ panic(\"recovered\"); }} finally {{ let resumed = 7; }} }}")).unwrap();
            let fatal = cli(&app, "run");
            assert_copy_fault(&fatal, kind);
            assert!(!String::from_utf8_lossy(&fatal.stderr).contains("ran buffer_app"));
            success(&cli(&app, "build"));
            let fatal_aot = Command::new(app.join("build/buffer_app")).output().unwrap();
            assert_copy_fault(&fatal_aot, kind);
            assert!(!String::from_utf8_lossy(&fatal_aot.stderr).contains("RINZU_RESULT="));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

fn assert_copy_fault(output: &Output, kind: i32) {
    if kind == 0 {
        assert_eq!(
            output.status.code(),
            Some(86),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        assert!(
            matches!(output.status.signal(), Some(10 | 11)),
            "kind={kind}, status={}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn instrumented(app: &Path, sanitized: bool) -> PathBuf {
    let runtime = std::fs::read_to_string(app.join("build/buffer_app.runtime.c")).unwrap();
    let string = "int status = rinzu_managed_receive_string(data, input, &received);";
    let bytes = "} else\n      rinzu_managed_receive_bytes(data, description->result_length, input, &received);";
    let error = "rinzu_failure_fail(&context->pending, status == RINZU_NATIVE_NULL_STRING ? 8 : 9,\n                           context->source);\n        return 0;";
    let retained = "rinzu_managed_set_count(&call->retained, frame->count + 1);";
    for hook in [string, bytes, error, retained] {
        assert_eq!(runtime.matches(hook).count(), 1, "copy probe hook: {hook}");
    }
    let runtime = runtime.replace(string, &format!("rinzu_test_arm(\"RINZU_TEST_TEXT_BUDGET\"); {string}"))
        .replace(bytes, "} else { rinzu_test_arm(\"RINZU_TEST_BYTES_BUDGET\"); rinzu_managed_receive_bytes(data, description->result_length, input, &received); }")
        .replace(error, "rinzu_failure_fail(&context->pending, status == RINZU_NATIVE_NULL_STRING ? 8 : 9, context->source);\n        rinzu_test_budget = -1; return 0;")
        .replace(retained, &format!("{retained}\n    rinzu_test_budget = -1;"));
    let runtime = format!(
        r#"#include <stdlib.h>
static _Thread_local long rinzu_test_budget = -1;
static void rinzu_test_arm(const char *name) {{
  const char *value = getenv(name); rinzu_test_budget = value ? strtol(value, NULL, 10) : -1;
}}
static int rinzu_test_allowed(void) {{
  if (!rinzu_test_budget) return 0;
  if (rinzu_test_budget > 0) --rinzu_test_budget;
  return 1;
}}
static void *rinzu_test_malloc(size_t size) {{ return rinzu_test_allowed() ? malloc(size) : NULL; }}
static void *rinzu_test_calloc(size_t count, size_t size) {{ return rinzu_test_allowed() ? calloc(count, size) : NULL; }}
static void *rinzu_test_realloc(void *pointer, size_t size) {{ return rinzu_test_allowed() ? realloc(pointer, size) : NULL; }}
#define malloc rinzu_test_malloc
#define calloc rinzu_test_calloc
#define realloc rinzu_test_realloc
{runtime}"#
    );
    let name = if sanitized {
        "copy-sanitized"
    } else {
        "copy-fault"
    };
    let source = app.join(format!("build/{name}.c"));
    let binary = app.join(format!("build/{name}"));
    std::fs::write(&source, runtime).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("clang");
    command
        .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
        .arg(app.join("build/buffer_app.o"))
        .arg(&source)
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

#[test]
fn native_data_copy_oom_is_fatal_and_error_notification_needs_no_extra_allocation() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-data-copy-fault-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(app.join("src/main.rinzu"), "import buffers; async fn main() { let result = buffers.exercise(b\"A\\xff\", \"日本語\\u{0}🙂\"); if result.caught != 7 { panic(\"wrong\"); } }").unwrap();
    success(&cli(&app, "build"));
    let marker = root.join("external-entry");
    for sanitized in [false, true] {
        let binary = instrumented(&app, sanitized);
        for inline_bytes in ["0", "1"] {
            let byte_budgets: &[i32] = if inline_bytes == "1" { &[0] } else { &[0, 1] };
            for (variable, budgets, entries) in [
                ("RINZU_TEST_TEXT_BUDGET", &[0, 1][..], 1),
                ("RINZU_TEST_BYTES_BUDGET", byte_budgets, 2),
            ] {
                for budget in budgets {
                    if marker.exists() {
                        std::fs::remove_file(&marker).unwrap();
                    }
                    let failed = Command::new(&binary)
                        .env(variable, budget.to_string())
                        .env("RINZU_MANAGED_INLINE_BYTES", inline_bytes)
                        .env("RINZU_TEST_DATA_ENTRY", &marker)
                        .env("ASAN_OPTIONS", "detect_leaks=0")
                        .output()
                        .unwrap();
                    assert_eq!(
                        failed.status.code(),
                        Some(71),
                        "{variable}={budget}: {}",
                        String::from_utf8_lossy(&failed.stderr)
                    );
                    assert_eq!(std::fs::read(&marker).unwrap(), vec![b'E'; entries]);
                    assert!(!String::from_utf8_lossy(&failed.stderr).contains("RINZU_RESULT="));
                }
            }
            let complete = Command::new(&binary)
                .env("RINZU_TEST_TEXT_BUDGET", "2")
                .env(
                    "RINZU_TEST_BYTES_BUDGET",
                    if inline_bytes == "1" { "1" } else { "2" },
                )
                .env("RINZU_MANAGED_INLINE_BYTES", inline_bytes)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            success(&complete);
            assert!(
                String::from_utf8_lossy(&complete.stderr)
                    .contains("RINZU_NATIVE_CALLS=18,18,18,18,18,18,32,128,0,0,0")
            );
        }
    }
    for (kind, budget, cause) in [(0, 0, 8), (3, 1, 9)] {
        std::fs::write(app.join("src/main.rinzu"), format!("import buffers; async fn main() {{ var caught = 0; try {{ let invalid = buffers.failure({kind}); }} catch inputError {{ caught = 1; }} if caught != 1 {{ panic(\"wrong\"); }} }}")).unwrap();
        success(&cli(&app, "build"));
        for sanitized in [false, true] {
            let binary = instrumented(&app, sanitized);
            let recovered = Command::new(&binary)
                .env("RINZU_TEST_TEXT_BUDGET", budget.to_string())
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            success(&recovered);
            let stdout = String::from_utf8_lossy(&recovered.stderr);
            assert!(
                stdout.contains(&format!("RINZU_CAUGHT_ERRORS={cause}@")),
                "{stdout}"
            );
            assert!(
                stdout.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0"),
                "{stdout}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
