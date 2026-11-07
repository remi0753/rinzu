#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn byte_payload_poison_covers_program_padding_and_gc() {
    generated_c_guards(
        "byte-initialization",
        include_str!("fixtures/bytes_initialization_guards.c"),
        &["padding", "program"],
        &[],
        "BYTES_INITIALIZATION",
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn byte_results_are_immutable_and_padded_in_jit_aot_and_worker_gc() {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/byte_construction.rinzu");
    for (inline, stress) in [("0", false), ("0", true), ("1", false), ("1", true)] {
        let output = std::process::Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&source)
            .env("RINZU_GC_STRESS", if stress { "1" } else { "0" })
            .env("RINZU_MANAGED_INLINE_BYTES", inline)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn concat_all_generated_c_guards_and_lifetimes_match_under_sanitizers() {
    generated_c_guards(
        "concat-all",
        include_str!("fixtures/bytes_concat_all_guards.c"),
        &[
            "normal",
            "empty",
            "one",
            "one-empty",
            "single",
            "parts63",
            "parts64",
            "parts65",
            "parts255",
            "parts256",
            "parts257",
            "parts1024",
            "zeros128",
            "zeros129",
            "zeros130",
            "zeros512",
            "zeros513",
            "zeros514",
            "empty129",
            "single129",
            "resize64",
            "resize256",
        ],
        &[
            "result",
            "scalar",
            "one-result",
            "one-scalar",
            "one-kind",
            "overflow",
            "trailing",
            "foreign",
            "late-scalar",
            "late-kind",
            "late-overflow",
            "late-foreign",
        ],
        "BYTES_CONCAT_ALL",
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn byte_array_generated_c_sharing_preserves_guards_snapshots_and_copy_lifetimes() {
    generated_c_guards(
        "byte-array-share",
        include_str!("fixtures/bytes_share_guards.c"),
        &["normal", "empty", "padding", "list"],
        &[
            "result",
            "length",
            "scalar",
            "kind",
            "foreign",
            "owner",
            "list-result",
        ],
        "BYTES_SHARE",
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn generated_c_guards(
    label: &str,
    guards: &str,
    valid_modes: &[&str],
    invalid_modes: &[&str],
    report: &str,
) {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-{label}-guards-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/byte_construction.rinzu");
    let built = Command::new(frozen_cli::path())
        .args(["build"])
        .arg(fixture)
        .arg("--output")
        .arg(root.join("fixture"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{built:?}");
    let source = fs::read_to_string(root.join("fixture.runtime.c")).unwrap();
    assert_eq!(source.matches("int main(").count(), 1);
    let source = source.replace("int main(", "int unused_source_main(") + guards;
    let source = if label == "byte-initialization" {
        String::from(include_str!("fixtures/bytes_poison_alloc.h")) + &source
    } else {
        source
    };
    fs::write(root.join("guards.c"), source).unwrap();
    for sanitized in [false, true] {
        let mut compile = Command::new("clang");
        compile
            .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(root.join("guards.c"))
            .arg(root.join("fixture.o"));
        if sanitized {
            compile.args(["-O1", "-g", "-fsanitize=address,undefined"]);
        } else {
            compile.arg("-O2");
        }
        let compiled = compile.arg("-o").arg(root.join("guards")).output().unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        for &mode in valid_modes.iter().chain(invalid_modes) {
            let inline_modes = if label == "byte-initialization" {
                vec![Some("0"), Some("1")]
            } else {
                vec![None]
            };
            for inline in inline_modes {
                let mut command = Command::new(root.join("guards"));
                command
                    .arg(mode)
                    .env("ASAN_OPTIONS", "detect_leaks=0")
                    .env("UBSAN_OPTIONS", "halt_on_error=1");
                if let Some(enabled) = inline {
                    command.env("RINZU_MANAGED_INLINE_BYTES", enabled);
                }
                let result = command.output().unwrap();
                let valid = valid_modes.contains(&mode);
                assert_eq!(
                    result.status.code(),
                    Some(if valid { 0 } else { 70 }),
                    "{mode}: {result:?}"
                );
                assert!(
                    !String::from_utf8_lossy(&result.stderr).contains("Sanitizer")
                        && !String::from_utf8_lossy(&result.stderr).contains("runtime error:"),
                    "{result:?}"
                );
                if valid {
                    assert_eq!(
                        String::from_utf8_lossy(&result.stdout).trim(),
                        format!("{report}={mode}")
                    );
                }
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}
