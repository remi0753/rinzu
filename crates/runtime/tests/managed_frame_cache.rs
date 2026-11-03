#![cfg(unix)]
use std::{path::Path, process::Command};

#[test]
fn c_domain_frame_reuse_preserves_live_chains_and_retained_accounting() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let work = std::env::temp_dir().join(format!("rinzu-frame-cache-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    for sanitizer in ["none", "address,undefined", "thread"] {
        for capacity in [64, 0, 2] {
            let binary = work.join("frames");
            let mut command = Command::new("clang");
            command
                .args([
                    "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
                ])
                .arg(repo.join("include"))
                .arg(format!("-DRINZU_MANAGED_FRAME_CACHE_COUNT={capacity}"))
                .arg(repo.join("crates/runtime/tests/fixtures/managed_frame_cache.c"));
            if sanitizer != "none" {
                command
                    .arg(format!("-fsanitize={sanitizer}"))
                    .arg("-fno-sanitize-recover=all");
            }
            let built = command.arg("-o").arg(&binary).output().unwrap();
            assert!(built.status.success(), "{built:?}");
            for stress in ["0", "1"] {
                let output = Command::new(&binary)
                    .env("RINZU_GC_STRESS", stress)
                    .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                    .env("UBSAN_OPTIONS", "halt_on_error=1")
                    .env("TSAN_OPTIONS", "halt_on_error=1")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{sanitizer}/{capacity}/{stress}: {output:?}"
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout)
                        .contains("retained-charged:dispose-zero")
                );
                assert!(output.stderr.is_empty(), "{output:?}");
            }
            if capacity == 64 && sanitizer != "thread" {
                for guard in ["foreign", "active-detach", "borrowed-dispose", "writing"] {
                    let output = Command::new(&binary).arg(guard).output().unwrap();
                    assert_eq!(output.status.code(), Some(70), "{guard}: {output:?}");
                    assert!(output.stderr.is_empty(), "{output:?}");
                }
            }
        }
    }
    std::fs::remove_dir_all(work).unwrap();
}
