#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn immutable_list_updates_match_source_jit_aot_worker_and_gc() {
    use std::{path::Path, process::Command};
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/list_updates.rinzu");
    for stress in ["0", "1"] {
        let output = Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&source)
            .env("RINZU_GC_STRESS", stress)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn list_updates_preserve_guards_gc_and_remove_scratch_allocation() {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-list-updates-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(manifest.join("tests/fixtures/list_updates.rinzu"))
        .arg("--output")
        .arg(root.join("fixture"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let source = fs::read_to_string(root.join("fixture.runtime.c")).unwrap();
    let direct = source.contains("rinzu_managed_list_update_owned(data, value,");
    assert_eq!(source.matches("int main(").count(), 1);
    let prelude = r#"
#include <stdlib.h>
#include <string.h>
static size_t list_allocations, list_copy_bytes, list_fail_at;
static int list_count_active;
static inline void *list_calloc(size_t n, size_t size) {
  if (list_count_active && ++list_allocations == list_fail_at) return NULL;
  return calloc(n, size);
}
static inline void *list_memcpy(void *out, const void *in, size_t n) {
  if (list_count_active) list_copy_bytes += n;
  return memcpy(out, in, n);
}
static inline void *list_memmove(void *out, const void *in, size_t n) {
  if (list_count_active) list_copy_bytes += n;
  return memmove(out, in, n);
}
#undef memcpy
#undef memmove
#define calloc list_calloc
#define memcpy list_memcpy
#define memmove list_memmove
"#;
    let source = format!(
        "#define TEST_LIST_DIRECT {}\n{}\n{}\n{}",
        usize::from(direct),
        prelude,
        source.replace("int main(", "int unused_source_main("),
        include_str!("fixtures/list_update_guards.c")
    );
    fs::write(root.join("guards.c"), source).unwrap();
    for sanitized in [false, true] {
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(manifest.join("../../include"))
            .arg(root.join("guards.c"))
            .arg(root.join("fixture.o"));
        if sanitized {
            command.args(["-O1", "-g", "-fsanitize=address,undefined"]);
        } else {
            command.arg("-O2");
        }
        let output = command.arg("-o").arg(root.join("guards")).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        for mode in [
            "normal",
            "scalar-tags",
            "references",
            "foreign-list",
            "foreign-value",
            "wrong-result",
            "wrong-element",
            "overflow",
            "owner",
            "oom-meta",
            "oom-slots",
            "oom-option",
        ] {
            let output = Command::new(root.join("guards"))
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap();
            let valid = ["normal", "scalar-tags", "references"].contains(&mode);
            assert_eq!(
                output.status.code(),
                Some(if valid { 0 } else { 70 }),
                "{mode}, sanitized={sanitized}: {output:?}"
            );
            assert!(output.stderr.is_empty(), "{output:?}");
        }
    }
    fs::remove_dir_all(root).unwrap();
}
