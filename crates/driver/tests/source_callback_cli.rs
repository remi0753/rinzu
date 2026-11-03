#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/checked_entry.rs"]
mod checked_entry;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
mod platform;
use std::process::Command;

#[test]
fn source_callback_first_event_can_stop_immediately_and_restart_without_late_callbacks() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-source_callback-first-event-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let app = platform::ordinary_import(&root);
    std::fs::write(
        app.join("src/main.rinzu"),
        checked_entry::source(
            r#"
import sample.platform;
class Count { value:Int }
async fn main() {
    let count=Count(value:0);
    let handler=fn(app:sample.platform.Handle,event:sample.platform.Event) fallback () {
        if event.kind!=1 || event.sequence!=1 || !sample.platform.isRunning(app) {
            panic("prematureEvent");
        }
        count.value=count.value+1;
        let stopped=sample.platform.stop(app);
        if stopped!=0 {panic("failedStop");}
    };
    var iteration=0;
    while iteration<3 {
        let app=sample.platform.create(true);
        let result=await sample.platform.run(app,handler);
        if result!=0 || sample.platform.isRunning(app) || count.value!=iteration+1 {panic("failedRun");}
        iteration=iteration+1;
    }
}
"#,
        ),
    )
    .unwrap();
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("run").arg(&app),
    ));
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/source_callback");
    let check = |output: std::process::Output| {
        platform::success(&output);
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(
            text.contains("RINZU_FAILURE=none") && text.contains("\"resources\":\"baseline\""),
            "{text}"
        );
        assert!(
            !text.contains("ERROR: AddressSanitizer") && !text.contains("runtime error:"),
            "{text}"
        );
    };
    check(platform::bounded(&mut Command::new(&binary)));
    let sanitized = platform::sanitize(&binary);
    check(platform::bounded(
        Command::new(&sanitized)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    check(platform::bounded(&mut Command::new(&binary)));
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("verify-aot").arg(&app),
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_null_returns_an_error_without_unwinding_the_native_call() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-source_callback-source-exception-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    let app = platform::ordinary_import(&root);
    std::fs::write(app.join("src/main.rinzu"),"import sample.platform; fn main(){var caught=0; try{let handle=sample.platform.create(false);}catch [error: Error, ...errorRest] {if error.message!=\"native value is NULL\"{panic(\"wrong\");}caught=caught+1;}finally{caught=caught+10;}if caught!=11 {panic(\"missing\");}}").unwrap();
    let app = root.join("app");
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("run").arg(&app),
    ));
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/source_callback");
    let check = |output: std::process::Output| {
        platform::success(&output);
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(
            text.contains("RINZU_FAILURE=none")
                && text.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,0,0,0,0,0"),
            "{text}"
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
    };
    check(platform::bounded(&mut Command::new(&binary)));
    let sanitized = platform::sanitize(&binary);
    check(platform::bounded(
        Command::new(&sanitized)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    check(platform::bounded(&mut Command::new(&binary)));
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("verify-aot").arg(&app),
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_callback_runs_typed_event_io_spawn_gc_and_stop_in_jit_aot_signed_and_sanitizers() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-source_callback-source-platform-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let app = platform::ordinary_import(&root);
    let jit = platform::bounded_tool(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .arg("--trace"),
    );
    platform::success(&jit);
    let stdout = String::from_utf8_lossy(&jit.stderr);
    assert!(
        stdout.contains("compiled JIT continuation runtime")
            && stdout.contains("\"resources\": \"baseline\""),
        "{stdout}"
    );
    let stderr = String::from_utf8_lossy(&jit.stderr);
    assert!(!stderr.contains("implemented in both"), "{stderr}");
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = app.join("build/source_callback");
    baseline(&platform::bounded(
        Command::new(&binary).env("RINZU_TRANSPORT_EVIDENCE", "1"),
    ));
    let sanitized = platform::sanitize(&binary);
    baseline(&platform::bounded(
        Command::new(&sanitized)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    ));
    platform::sign(&binary, &root);
    baseline(&platform::bounded(
        Command::new(&binary).env("RINZU_TRANSPORT_EVIDENCE", "1"),
    ));
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("verify-aot").arg(&app),
    ));
    std::fs::remove_dir_all(root).unwrap();
}

fn baseline(output: &std::process::Output) {
    platform::success(output);
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        report.contains("RINZU_FAILURE=none") && report.contains("\"resources\":\"baseline\""),
        "{report}"
    );
    assert!(
        report.contains("RINZU_NATIVE_CALLS=5,5,5,5,5,5,1,0,0,0,0"),
        "{report}"
    );
    assert!(
        report.contains("RINZU_STARTED_CHILDREN=1,1")
            && report.contains("file-read-size")
            && report.contains("spawn"),
        "{report}"
    );
    assert!(
        !report.contains("ERROR: AddressSanitizer") && !report.contains("runtime error:"),
        "{report}"
    );
}
