#![cfg(unix)]
use rinzu_runtime::{failure::SourcePosition, native_diagnostics::ActivityMonitor};
#[test]
fn prepared_wait_guards_preserve_acyclic_dependencies_and_remove_old_cancelled_edges() {
    let monitor = ActivityMonitor::new();
    let source = SourcePosition {
        file: 17,
        start_byte: 31,
        end_byte: 49,
    };
    let first = monitor.wait_for(1, 2, c"first.await", source, 41, 2);
    let second = monitor.wait_for(2, 3, c"second.await", source, 41, 2);
    assert_eq!(monitor.stats().wait_live, 2);
    assert!(monitor.stats().wait_bytes > 0 && !monitor.stats().is_quiescent());
    drop(second);
    let changed = monitor.wait_for(2, 4, c"changed.await", source, 41, 2);
    drop(first);
    drop(changed);
    assert!(monitor.stats().is_quiescent());
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn signed_and_sanitized_observable_cycles_fail_without_synthetic_return_or_cleanup() {
    use std::{path::Path, process::Command};
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = std::env::temp_dir().join(format!("rinzu-wait-dependency-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread",
            ])
            .arg(manifest.join("tests/fixtures/wait_dependency.c"))
            .arg("-I")
            .arg(manifest.join("../../include"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        if !sanitized {
            for args in [
                ["--force", "--sign", "-", "--options", "runtime"].as_slice(),
                ["--verify", "--strict"].as_slice(),
            ] {
                let signed = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(signed.status.success(), "{signed:?}");
            }
            let entitlements = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(entitlements.status.success() && entitlements.stdout.is_empty());
        }
        let run = |mode| {
            Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap()
        };
        let normal = run("normal");
        assert!(normal.status.success(), "{normal:?}");
        assert!(String::from_utf8_lossy(&normal.stdout).contains("PASS observed wait"));
        let diagnostic = String::from_utf8_lossy(&normal.stderr);
        assert!(
            diagnostic.contains("long-native symbol=unobserved.native.lock")
                && !diagnostic.contains("observable-wait-cycle"),
            "{diagnostic}"
        );
        for mode in ["self", "cycle", "long-cycle"] {
            let negative = run(mode);
            assert_eq!(negative.status.code(), Some(86), "{negative:?}");
            assert!(negative.stdout.is_empty(), "{negative:?}");
            let diagnostic = String::from_utf8_lossy(&negative.stderr);
            assert!(
                diagnostic.contains("rinzu fatal: observable-wait-cycle")
                    && diagnostic.contains("source=17:31:49 owner=0x29 depth=2"),
                "{diagnostic}"
            );
            assert!(!diagnostic.contains("runtime error:"), "{diagnostic}");
            if mode == "cycle" {
                for name in [
                    "first.await",
                    "second.await",
                    "closing.await",
                    "native.join",
                ] {
                    assert!(diagnostic.contains(name), "{diagnostic}");
                }
                assert_eq!(
                    diagnostic
                        .lines()
                        .filter(|line| line.starts_with("rinzu wait: "))
                        .count(),
                    3
                );
            }
        }
        for (mode, status) in [
            ("oom-wait", 71),
            ("duplicate", 86),
            ("wrong-thread", 86),
            ("pinned-board", 86),
        ] {
            let negative = run(mode);
            assert_eq!(negative.status.code(), Some(status), "{mode}: {negative:?}");
            assert!(
                negative.stdout.is_empty() && negative.stderr.is_empty(),
                "{negative:?}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
