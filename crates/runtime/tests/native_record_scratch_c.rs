#![cfg(unix)]
use std::path::Path;
use std::process::Command;
#[test]
fn c_native_record_scratch_has_zero_warm_allocations_and_bounded_reference_free_retention() {
    let root = std::env::temp_dir().join(format!("rinzu-c-native-scratch-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "normal" });
        let mut command = Command::new("clang");
        command
            .args([
                "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/native_record_scratch.c"));
        if sanitized {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = command.arg("-o").arg(&binary).output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(&binary)
            .env_remove("RINZU_GC_STRESS")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("PASS C native scratch"));
        assert!(output.stderr.is_empty(), "{output:?}");
        for row in text
            .lines()
            .filter_map(|line| line.strip_prefix("NATIVE_RECORD_SCRATCH="))
        {
            let mut value: serde_json::Value = serde_json::from_str(row).unwrap();
            value["sanitized"] = sanitized.into();
            eprintln!("NATIVE_RECORD_SCRATCH={value}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
