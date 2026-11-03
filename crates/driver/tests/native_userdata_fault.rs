#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
#[path = "fixtures/native_userdata_sdk/setup.rs"]
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
    let mut runtime = std::fs::read_to_string(app.join("build/userdata_app.runtime.c")).unwrap();
    runtime = replace(
        runtime,
        "call->userdata_scope = rinzu_userdata_enter(rinzu_context_userdata(context), data);\n      break;",
        "rinzu_test_arm(\"RINZU_TEST_USERDATA_SCOPE\"); call->userdata_scope=rinzu_userdata_enter(rinzu_context_userdata(context),data); rinzu_test_budget=-1; break;",
    );
    runtime = replace(
        runtime,
        "call->words[index] = (uint64_t)(uintptr_t)rinzu_userdata_prepare(\n          rinzu_context_userdata(context), data, call->userdata_scope, (size_t)parameter->length,\n          bits);",
        "const char *selected=getenv(\"RINZU_TEST_USERDATA_ARGUMENT\"); if (selected && index==(size_t)strtoull(selected,NULL,10)) rinzu_test_arm(\"RINZU_TEST_USERDATA_PREPARE\"); call->words[index]=(uint64_t)(uintptr_t)rinzu_userdata_prepare(rinzu_context_userdata(context),data,call->userdata_scope,(size_t)parameter->length,bits); rinzu_test_budget=-1;",
    );
    runtime = replace(
        runtime,
        "call->phase = 1;\n  if (call->bounded)",
        "call->phase = 1; rinzu_test_marker('R'); rinzu_test_arm(\"RINZU_TEST_USERDATA_RETURN\");\n  if (call->bounded)",
    );
    runtime = replace(
        runtime,
        "free(call);\n}\n#ifdef RINZU_AOT_HAS_SUPPLANT",
        "free(call); rinzu_test_budget=-1;\n}\n#ifdef RINZU_AOT_HAS_SUPPLANT",
    );
    runtime = replace(
        runtime,
        "  rinzu_aot_source_native_startup();",
        "  rinzu_aot_source_native_startup(); if (pthread_once(&rinzu_program_schema_once,rinzu_init_program_schema)) _Exit(90); rinzu_test_baseline_count=rinzu_test_count; rinzu_test_baseline_bytes=rinzu_test_bytes;",
    );
    runtime = replace(
        runtime,
        "return exit_code;",
        "rinzu_userdata_process_stats_t process=rinzu_userdata_process_stats(); if (rinzu_test_count!=rinzu_test_baseline_count+process.tokens || rinzu_test_bytes!=rinzu_test_baseline_bytes+process.token_bytes) _Exit(90); puts(\"PASS userdata allocation baseline restored without token metadata\"); return exit_code;",
    );
    let prelude = include_str!("fixtures/native_userdata_sdk/allocator.h");
    let source = app.join(format!(
        "build/userdata-{}.c",
        if sanitized { "sanitized" } else { "fault" }
    ));
    let binary = source.with_extension("");
    std::fs::write(&source, format!("{prelude}\n{runtime}")).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut clang = Command::new("clang");
    clang
        .args(["-std=c11", "-O1", "-g", "-Werror", "-pthread"])
        .arg(app.join("build/userdata_app.o"))
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

fn probe(binary: &Path, marker: &Path, env: &[(&str, String)]) -> Output {
    let _ = std::fs::remove_file(marker);
    let mut command = Command::new(binary);
    command
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("RINZU_TEST_USERDATA_ENTRY", marker);
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().unwrap()
}
fn entry(marker: &Path) -> Vec<u8> {
    std::fs::read(marker).unwrap_or_default()
}

#[test]
fn userdata_actual_call_oom_skips_entry_and_return_teardown_allocates_nothing() {
    let root =
        std::env::temp_dir().join(format!("rinzu-userdata-call-fault-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let api = root.join("userdata/api.c");
    let original = std::fs::read_to_string(&api).unwrap();
    assert_eq!(original.matches("  ++entries;").count(), 1);
    let marker = r#"#include <stdlib.h>
#include <unistd.h>
#include <fcntl.h>
static void userdata_marker(void) {const char *path=getenv("RINZU_TEST_USERDATA_ENTRY");if (!path) return;int file=open(path,O_WRONLY|O_CREAT|O_APPEND,0600);if (file<0 || write(file,"E",1)!=1 || close(file)) _Exit(90);}
"#;
    std::fs::write(
        api,
        format!(
            "{marker}\n{}",
            original.replace("  ++entries;", "    ++entries; userdata_marker();")
        ),
    )
    .unwrap();
    std::fs::write(app.join("src/main.rinzu"),"import userdata; async fn main() {try {let value=userdata.returning();if value.id!=41 {panic(\"wrong\");}}catch incorrectlyRecovered {panic(\"invalid\");}}\n").unwrap();
    success(
        &Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .output()
            .unwrap(),
    );
    let marker = root.join("entry-marker");
    for sanitized in [false, true] {
        let binary = instrument(&app, sanitized);
        for budget in [0, 1] {
            let output = probe(
                &binary,
                &marker,
                &[("RINZU_TEST_USERDATA_SCOPE", budget.to_string())],
            );
            assert_eq!(
                output.status.code(),
                Some(71),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(entry(&marker).is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("RINZU_RESULT="));
        }
        for argument in 0..5 {
            let mut completed = false;
            let mut failed_points = 0;
            // Aliased arguments may reuse prepared data. Exercise every real
            // allocation before the first success, without fixing its cost.
            for budget in 0..16 {
                let output = probe(
                    &binary,
                    &marker,
                    &[
                        ("RINZU_TEST_USERDATA_ARGUMENT", argument.to_string()),
                        ("RINZU_TEST_USERDATA_PREPARE", budget.to_string()),
                    ],
                );
                if output.status.success() {
                    assert_eq!(entry(&marker), b"ER");
                    completed = true;
                    break;
                }
                failed_points += 1;
                assert_eq!(
                    output.status.code(),
                    Some(71),
                    "argument={argument},budget={budget}: {output:?}"
                );
                assert!(entry(&marker).is_empty());
                assert!(!String::from_utf8_lossy(&output.stderr).contains("RINZU_RESULT="));
            }
            assert!(completed && failed_points > 0, "argument {argument}");
            let output = probe(
                &binary,
                &marker,
                &[
                    ("RINZU_TEST_USERDATA_SCOPE", "2".into()),
                    ("RINZU_TEST_USERDATA_ARGUMENT", argument.to_string()),
                    ("RINZU_TEST_USERDATA_PREPARE", "16".into()),
                    ("RINZU_TEST_USERDATA_RETURN", "0".into()),
                ],
            );
            success(&output);
            assert_eq!(entry(&marker), b"ER");
            let stdout = String::from_utf8_lossy(&output.stderr);
            let witness = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains("RINZU_NATIVE_CALLS=1,1,1,1,1,1,32,128,0,0,0")
                    && stdout.contains("RINZU_NATIVE_USERDATA=0,0,0,0,5,5")
                    && stdout.contains("RINZU_USERDATA_PROCESS=0,0,0")
                    && witness.contains("PASS userdata allocation baseline"),
                "{stdout}\n{witness}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
