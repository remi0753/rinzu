#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn virtual_slice_payloads_preserve_c_jit_and_worker_roots() {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-virtual-jit-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/virtual_bytes_option.rinzu");
    for stress in ["0", "1"] {
        let report = root.join(format!("resources-{stress}.jsonl"));
        let result = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&source)
            .arg("--resource-report")
            .arg(&report)
            .args(["--resource-interval-ms", "0"])
            .env("RINZU_GC_STRESS", stress)
            .env("RINZU_ERROR_TRACE", "1")
            .output()
            .unwrap();
        assert!(result.status.success(), "stress={stress}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("compiled JIT continuation runtime")
        );
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("C06:bytes-option:bounds:escape:gc:worker:alias:OK")
        );
        let rows = fs::read_to_string(&report).unwrap();
        let last: serde_json::Value = serde_json::from_str(rows.lines().last().unwrap()).unwrap();
        for category in last["categories"].as_array().unwrap() {
            if !matches!(
                category["id"].as_str(),
                Some("rss" | "virtual" | "footprint")
            ) {
                assert_eq!(category["live"].as_u64().unwrap_or(0), 0, "{category}");
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn generated_slice_payloads_preserve_owner_oom_and_moving_gc_guards() {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-virtual-guards-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let built = Command::new(frozen_cli::path())
        .arg("build")
        .arg(repo.join("crates/driver/tests/fixtures/virtual_bytes_option_kernel.rinzu"))
        .arg("--output")
        .arg(root.join("program"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let source = format!(
        "{}\n#define main c06_generated_main\n#include {:?}\n#undef main\n{}",
        include_str!("fixtures/virtual_bytes_option_alloc.h"),
        root.join("program.runtime.c").to_str().unwrap(),
        include_str!("fixtures/virtual_bytes_option_guards.c"),
    );
    fs::write(root.join("guards.c"), source).unwrap();
    for sanitized in [false, true] {
        let mut command = Command::new("clang");
        command
            .args([
                "-target",
                "arm64-apple-macos11",
                "-std=c11",
                "-O2",
                "-pthread",
                "-I",
            ])
            .arg(repo.join("include"))
            .arg(root.join("guards.c"))
            .arg(root.join("program.o"))
            .arg(repo.join("crates/runtime/src/callback_entry.c"))
            .arg(repo.join("crates/runtime/src/callback_template.S"));
        if sanitized {
            command.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let built = command.arg("-o").arg(root.join("guards")).output().unwrap();
        assert!(built.status.success(), "{built:?}");
        for mode in [
            "normal",
            "oom1",
            "oom2",
            "oom3",
            "legacy-oom1",
            "legacy-oom2",
            "legacy-oom3",
            "owner",
            "foreign",
            "zero",
            "kind",
            "overflow",
            "negative-type",
            "large-type",
            "scalar-type",
            "stale",
            "borrow",
            "writing",
        ] {
            let result = Command::new(root.join("guards"))
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            let expected = if matches!(mode, "normal" | "oom3") {
                0
            } else {
                70
            };
            assert_eq!(result.status.code(), Some(expected), "{mode}: {result:?}");
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(
                !stderr.contains("Sanitizer") && !stderr.contains("runtime error:"),
                "{result:?}"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}
