#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "fixtures/native_opaque_sdk/setup.rs"]
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
fn opaque_cli_runs_compiled_owners_and_catches_null_at_checked_roots() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-native-opaque-{}", std::process::id()));
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
    let binary = app.join("build/opaque_app");
    let aot = Command::new(&binary).output().unwrap();
    success(&aot);
    assert!(
        String::from_utf8_lossy(&aot.stderr)
            .contains("RINZU_NATIVE_CALLS=351,351,351,351,351,351,288,1152,0,0,0")
    );
    success(&cli(&app, "verify-aot"));

    std::fs::write(app.join("src/main.rinzu"), "import objects; fn main() { if objects.measure(42) != 42 { panic(\"wrong\"); } var caught = 0; try { let missing = objects.failure(); } catch nullError { caught = 1; } if caught != 1 { panic(\"notCaught\"); } }").unwrap();
    let compiled = cli(&app, "run");
    success(&compiled);
    assert!(String::from_utf8_lossy(&compiled.stderr).contains("baseline JIT"));
    success(&cli(&app, "verify-aot"));

    for main in ["fn main()", "async fn main()"] {
        std::fs::write(
            app.join("src/main.rinzu"),
            format!("import objects; {main} {{let missing=objects.failure();}}"),
        )
        .unwrap();
        let rejected = cli(&app, "check");
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("RZ0413"));
        std::fs::write(app.join("src/main.rinzu"), format!(r#"import objects; {main} {{
            var caught=false;
            try {{let missing=objects.failure();}}
            catch [problem:Error,...rest] {{caught=true;if problem.message!="native value is NULL"||listLength(rest)!=0{{panic("wrongError");}}}}
            if !caught{{panic("missingError");}}
        }}"#)).unwrap();
        success(&cli(&app, "run"));
        success(&cli(&app, "build"));
        let aot = Command::new(&binary).output().unwrap();
        success(&aot);
        let report = String::from_utf8_lossy(&aot.stderr);
        assert!(report.contains("RINZU_CAUGHT_ERRORS=8@"), "{report}");
        assert!(report.contains("RINZU_FAILURE=none"), "{report}");
        assert!(
            report.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0"),
            "{report}"
        );
        success(&cli(&app, "verify-aot"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

fn compile_instrumented(app: &Path, source: &str, sanitized: bool) -> PathBuf {
    let name = if sanitized {
        "reservation-sanitized"
    } else {
        "reservation-fault"
    };
    let runtime = app.join(format!("build/{name}.c"));
    let binary = app.join(format!("build/{name}"));
    std::fs::write(&runtime, source).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut command = Command::new("clang");
    command
        .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
        .arg(app.join("build/opaque_app.o"))
        .arg(&runtime)
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
fn opaque_aot_reservation_faults_stop_before_entry_and_read_needs_no_allocation() {
    let root = std::env::temp_dir().join(format!("rinzu-cli-opaque-fault-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { if objects.measure(42) != 42 { panic(\"wrong\"); } var caught = 0; try { let absent = objects.failure(); } catch nullError { caught = 1; } if caught != 1 { panic(\"missingError\"); } }").unwrap();
    success(&cli(&app, "build"));
    let runtime = std::fs::read_to_string(app.join("build/opaque_app.runtime.c")).unwrap();
    let reserve = "call->opaque_result = rinzu_managed_reserve_opaque(data, plan->result_type);";
    assert_eq!(runtime.matches(reserve).count(), 1);
    let receive = "int status = rinzu_managed_receive_opaque(";
    assert_eq!(runtime.matches(receive).count(), 1);
    // Private test-only interposition is armed at the real ledger's reservation
    // and result read, after immutable startup preparation has completed.
    let hooked = runtime.replace(reserve, &format!("const char *fault = getenv(\"RINZU_TEST_RESERVATION_BUDGET\"); if (fault) rinzu_test_budget = strtol(fault, NULL, 10); {reserve}"))
        .replace("rinzu_managed_publish(&call->retained, roots, (int64_t)root_count);\n    free(roots);", "rinzu_managed_publish(&call->retained, roots, (int64_t)root_count); free(roots); rinzu_test_budget = -1;")
        .replace(receive, &format!("rinzu_test_budget = 0; {receive}"))
        .replace("rinzu_failure_fail(&context->pending, 8, context->source);\n      return 0;", "rinzu_failure_fail(&context->pending, 8, context->source);\n      rinzu_test_budget = -1; return 0;")
        .replace("    return received;", "    rinzu_test_budget = -1; return received;");
    let hooked = format!(
        r#"#include <stdlib.h>
static _Thread_local long rinzu_test_budget = -1;
static int rinzu_test_allocation_allowed(void) {{
  if (!rinzu_test_budget) return 0;
  if (rinzu_test_budget > 0) --rinzu_test_budget;
  return 1;
}}
static void *rinzu_test_calloc(size_t count, size_t size) {{
  if (!rinzu_test_allocation_allowed()) return NULL;
  return calloc(count, size);
}}
static void *rinzu_test_malloc(size_t size) {{
  if (!rinzu_test_allocation_allowed()) return NULL;
  return malloc(size);
}}
static void *rinzu_test_realloc(void *pointer, size_t size) {{
  if (!rinzu_test_allocation_allowed()) return NULL;
  return realloc(pointer, size);
}}
#define calloc rinzu_test_calloc
#define malloc rinzu_test_malloc
#define realloc rinzu_test_realloc
{hooked}"#
    );
    let marker = root.join("external-entry");
    for sanitized in [false, true] {
        let binary = compile_instrumented(&app, &hooked, sanitized);
        for budget in [0, 1] {
            if marker.exists() {
                std::fs::remove_file(&marker).unwrap();
            }
            let failed = Command::new(&binary)
                .env("RINZU_TEST_RESERVATION_BUDGET", budget.to_string())
                .env("RINZU_TEST_OPAQUE_ENTRY", &marker)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert_eq!(
                failed.status.code(),
                Some(71),
                "budget={budget}: {}",
                String::from_utf8_lossy(&failed.stderr)
            );
            assert!(
                !marker.exists(),
                "external entry occurred at budget={budget}"
            );
            assert!(!String::from_utf8_lossy(&failed.stderr).contains("RINZU_RESULT="));
        }
        let received = Command::new(&binary)
            .env_remove("RINZU_TEST_RESERVATION_BUDGET")
            .env("RINZU_TEST_OPAQUE_ENTRY", &marker)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        success(&received);
        assert_eq!(std::fs::read(&marker).unwrap(), b"EE");
        assert!(
            String::from_utf8_lossy(&received.stderr)
                .contains("RINZU_NATIVE_CALLS=39,39,39,39,39,39,32,128,0,0,0")
        );
        std::fs::remove_file(&marker).unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}
