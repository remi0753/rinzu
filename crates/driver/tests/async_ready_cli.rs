#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn ready_snapshots_preserve_creation_order_aliases_and_gc() {
    use std::{path::Path, process::Command};
    for name in [
        "async_ready_order.rinzu",
        "async_ready_alias.rinzu",
        "async_task_environment.rinzu",
        "async_result_roots.rinzu",
        "async_gc_survivor.rinzu",
        "virtual_bytes_option.rinzu",
        "dead_temporary_roots.rinzu",
        "dead_local_roots.rinzu",
        "fresh_return_consumption.rinzu",
    ] {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        for stress in ["0", "1"] {
            let result = Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&source)
                .env("RINZU_GC_STRESS", stress)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{name}, stress={stress}: {result:?}"
            );
            assert!(String::from_utf8_lossy(&result.stdout).contains("outcome=success"));
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn generated_ready_snapshots_survive_nested_resume_and_retired_handles() {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-ready-guards-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/async_ready_order.rinzu");
    let built = Command::new(frozen_cli::path())
        .arg("build")
        .arg(source)
        .arg("--output")
        .arg(root.join("fixture"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let source = fs::read_to_string(root.join("fixture.runtime.c")).unwrap();
    let wrapper = source
        .find("static void rinzu_aot_async_resume_ready(")
        .unwrap();
    let begin = source
        .find("static __attribute__((noinline)) void rinzu_aot_async_resume_snapshot(")
        .unwrap_or(wrapper);
    let end = source[wrapper..].find("\n}\n").unwrap() + wrapper + 3;
    let body = &source[begin..end];
    let indexed = source.contains("static void rinzu_aot_async_queue_ready(");
    let resume = "rinzu_aot_async_resume(";
    assert_eq!(
        body.matches(resume).count(),
        if begin == wrapper { 1 } else { 2 }
    );
    let source = format!(
        "#define TEST_READY_INDEX {}\n{}\nstatic __attribute__((noinline)) void ready_probe_resume(RinzuContext *, RinzuAsyncTask *);\n{}{}{}",
        usize::from(indexed),
        &source[..begin],
        body.replace(resume, "ready_probe_resume("),
        &source[end..],
        include_str!("fixtures/ready_snapshot_guards.c"),
    );
    assert_eq!(source.matches("int main(").count(), 2);
    let source = source.replacen("int main(", "int unused_source_main(", 1);
    fs::write(root.join("guards.c"), source).unwrap();
    for sanitized in [false, true] {
        let mut command = Command::new("clang");
        command
            // Whole-function C lowering can retain unreachable block labels.
            // Keep strict diagnostics for the guard without rejecting valid
            // generated code for those labels (also present in baseline100).
            .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror", "-Wno-unused-label", "-I"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(root.join("guards.c"))
            .arg(root.join("fixture.o"));
        if sanitized {
            command.args(["-O1", "-g", "-fsanitize=address,undefined"]);
        } else {
            command.arg("-O2");
        }
        let built = command.arg("-o").arg(root.join("guards")).output().unwrap();
        assert!(built.status.success(), "{built:?}");
        for mode in [
            "empty",
            "forward",
            "reverse",
            "mixed",
            "mixed-sparse",
            "deferred",
            "deferred-reentrant",
            "reentrant",
            "retired",
            "control",
        ] {
            let result = Command::new(root.join("guards"))
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert!(result.status.success(), "{mode}: {result:?}");
            assert_eq!(
                String::from_utf8_lossy(&result.stdout).trim(),
                format!("READY_SNAPSHOT={mode}")
            );
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.starts_with("READY_SIZES:"), "{result:?}");
            assert!(
                !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
                "{result:?}"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}
