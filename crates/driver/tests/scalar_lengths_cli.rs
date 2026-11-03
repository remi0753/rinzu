#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn scalar_lengths_match_source_jit_aot_worker_and_gc() {
    use std::{path::Path, process::Command};
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scalar_lengths.rinzu");
    for stress in ["0", "1"] {
        let jit = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&source)
            .env("RINZU_GC_STRESS", stress)
            .output()
            .unwrap();
        assert!(jit.status.success(), "CLI JIT stress={stress}: {jit:?}");
        assert_eq!(jit.stdout, b"scalar-lengths-ok\n");
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&source)
            .env("RINZU_GC_STRESS", stress)
            .output()
            .unwrap();
        assert!(output.status.success(), "stress={stress}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn scalar_length_entries_preserve_operand_guards_and_borrow_lifetimes() {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-scalar-lengths-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(manifest.join("tests/fixtures/scalar_lengths.rinzu"))
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
            "{}\n{}",
            source.replace("int main(", "int unused_source_main("),
            include_str!("fixtures/scalar_length_guards.c")
        ),
    )
    .unwrap();
    for sanitized in [false, true] {
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(manifest.join("../../include"))
            .arg(root.join("guards.c"))
            .arg(root.join("fixture.o"));
        command.args(if sanitized {
            vec!["-O1", "-g", "-fsanitize=address,undefined"]
        } else {
            vec!["-O2"]
        });
        let output = command.arg("-o").arg(root.join("guards")).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        for mode in [
            "normal",
            "byte-kind",
            "string-kind",
            "list-kind",
            "list-type",
            "map-type",
            "negative-type",
            "past-type",
            "variant",
            "odd-map",
            "byte-overflow",
            "string-overflow",
            "list-overflow",
            "foreign-byte",
            "foreign-string",
            "foreign-list",
            "stale",
            "zero",
            "owner-byte",
            "owner-string",
            "owner-list",
        ] {
            let output = Command::new(root.join("guards"))
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode == "normal" { 0 } else { 70 }),
                "{mode}, sanitized={sanitized}: {output:?}"
            );
            assert!(output.stderr.is_empty(), "{output:?}");
        }
    }
    fs::remove_dir_all(root).unwrap();
}
