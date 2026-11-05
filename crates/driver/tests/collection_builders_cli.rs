#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

use std::{fs, path::Path, process::Command};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn list_and_option_helpers_preserve_callback_semantics() {
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(fixture("collection_helpers.rinzu"))
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
}

#[test]
fn bulk_operations_and_private_updates_match_jit_and_aot_under_moving_gc() {
    for name in ["collection_builders.rinzu", "collection_futures.rinzu"] {
        for stress in ["0", "1"] {
            let output = Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(fixture(name))
                .env("RINZU_GC_STRESS", stress)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{name} (GC stress {stress}): {output:?}"
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
        }
    }
    // An update is still one logical allocation even when the buffer stays in place.
    let output = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(fixture("collection_builders.rinzu"))
        .args(["--max-allocations", "80"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn generated_c_builders_move_only_on_growth_and_check_the_signed_count_limit() {
    let root = std::env::temp_dir().join(format!("rinzu-collection-probe-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let output = Command::new(frozen_cli::path())
        .arg("build")
        .arg(fixture("collection_builders.rinzu"))
        .arg("--output")
        .arg(root.join("fixture"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let source = fs::read_to_string(root.join("fixture.runtime.c")).unwrap();
    let instrument = r#"
#define RINZU_COLLECTION_PROBE
#define RINZU_MANAGED_METADATA_CACHE_CAPACITY 0
#include <stdlib.h>
#include <string.h>
static size_t copy_bytes, allocations, hashes;
static int measuring;
static inline void *counted_memcpy(void *out, const void *in, size_t n) {
  if (measuring) copy_bytes += n;
  return memcpy(out, in, n);
}
static inline void *counted_calloc(size_t n, size_t size) {
  if (measuring) allocations++;
  return calloc(n, size);
}
#undef memcpy
#define memcpy counted_memcpy
#define calloc counted_calloc
"#;
    // The private constructor entry reuses the production hash function.
    // Count calls at that entry independently from index relocation.
    let hash_probe = r#"
static inline int counted_hash(rinzu_managed_data_t *data, size_t type,
                               rinzu_managed_slot_t slot, uint64_t *hash) {
  if (measuring) hashes++;
  return rinzu_managed_value_hash(data, type, slot, hash);
}
#define rinzu_managed_value_hash counted_hash
"#;
    let marker = "static __attribute__((noinline, cold)) int64_t rinzu_aot_collection_overflow";
    assert!(source.contains(marker));
    let source = source
        .replacen(marker, &format!("{hash_probe}{marker}"), 1)
        .replacen("int main(", "int unused_source_main(", 1);
    fs::write(
        root.join("probe.c"),
        format!(
            "{instrument}\n{source}\n{}",
            include_str!("fixtures/collection_builders.c")
        ),
    )
    .unwrap();
    let output = Command::new("clang")
        .args([
            "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
        ])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(root.join("probe.c"))
        .arg(root.join("fixture.o"))
        .arg("-o")
        .arg(root.join("probe"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = Command::new(root.join("probe")).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    fs::remove_dir_all(root).unwrap();
}
