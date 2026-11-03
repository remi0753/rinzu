#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn observed_ledgers_keep_future_aliases_results_failures_and_physical_pins() {
    use std::{fs, path::Path, process::Command};
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest.join("../codegen/tests/fixtures/observed-ledger-alias.rinzu");
    for stress in ["0", "1"] {
        let output = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&fixture)
            .env("RINZU_GC_STRESS", stress)
            .output()
            .unwrap();
        assert!(output.status.success(), "JIT stress={stress}: {output:?}");
        assert_eq!(output.stdout, b"returned-future-ok\n");
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&fixture)
            .env("RINZU_GC_STRESS", stress)
            .output()
            .unwrap();
        assert!(output.status.success(), "AOT stress={stress}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
    let root = std::env::temp_dir().join(format!("rinzu-observed-ledger-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(&fixture)
        .arg("--output")
        .arg(root.join("fixture"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let source = fs::read_to_string(root.join("fixture.runtime.c")).unwrap();
    assert_eq!(source.matches("int main(").count(), 1);
    fs::write(
        root.join("guards.c"),
        format!(
            "#define TEST_LEDGER_POLICY 2\n{}\n{}",
            source.replace("int main(", "int unused_source_main("),
            include_str!("fixtures/observed_ledger_guards.c")
        ),
    )
    .unwrap();
    for flags in [
        vec!["-O2"],
        vec![
            "-O2",
            "-fsanitize=address,undefined",
            "-fno-sanitize-recover=all",
        ],
        vec!["-O2", "-fsanitize=thread"],
    ] {
        let output = Command::new("clang")
            .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(manifest.join("../../include"))
            .args(&flags)
            .arg(root.join("guards.c"))
            .arg(root.join("fixture.o"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg("-o")
            .arg(root.join("guards"))
            .output()
            .unwrap();
        assert!(output.status.success(), "{flags:?}: {output:?}");
        let output = Command::new(root.join("guards"))
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .env("TSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{flags:?}: {output:?}");
        assert_eq!(output.stdout, b"OBSERVED_LEDGER_ALIAS_PINS=6:ok\n");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    fs::remove_dir_all(root).unwrap();
}
