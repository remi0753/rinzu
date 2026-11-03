#![cfg(unix)]
use std::{path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("rinzu-activity-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

fn compile(fixture: &str, binary: &std::path::Path, flags: &[&str]) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = Command::new("clang")
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"])
        .args(flags)
        .arg("-I")
        .arg(root.join("include"))
        .arg(root.join("crates/runtime/tests/fixtures").join(fixture))
        .arg("-o")
        .arg(binary)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

fn run(binary: &std::path::Path, args: &[&str], expected: i32) -> std::process::Output {
    let out = Command::new(binary)
        .args(args)
        .env("ASAN_OPTIONS", "detect_leaks=0:halt_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1")
        .env("TSAN_OPTIONS", "halt_on_error=1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(expected), "{args:?}: {out:?}");
    let diagnostics = String::from_utf8_lossy(&out.stderr);
    for message in [
        "ERROR: AddressSanitizer",
        "runtime error:",
        "WARNING: ThreadSanitizer",
    ] {
        assert!(!diagnostics.contains(message), "{out:?}");
    }
    out
}

#[test]
fn diagnostic_gate_preserves_exclusion_handoff_condition_wait_gaps_and_fallback() {
    let dir = scratch("gate");
    let mut variants: Vec<(&str, Vec<&str>)> = vec![
        ("plain", vec![]),
        (
            "san",
            vec![
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
        ),
        ("fallback", vec!["-DRINZU_ACTIVITY_GATE_LEASE=0"]),
    ];
    if cfg!(target_os = "macos") {
        variants.push((
            "tsan",
            vec!["-g", "-fsanitize=thread", "-fno-sanitize-recover=all"],
        ));
    }
    for (name, flags) in variants {
        let binary = dir.0.join(name);
        compile("activity_gate.c", &binary, &flags);
        let out = run(&binary, &[], 0);
        let diagnostics = String::from_utf8_lossy(&out.stderr);
        assert!(diagnostics.contains("65536transactions"), "{out:?}");
        if diagnostics.contains("lease=1:") {
            for guard in ["unlocked", "double", "state", "wait"] {
                run(&binary, &[guard], 86);
            }
        }
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn diagnostic_gate_maps_new_handoff_and_condition_errors_to_existing_fatal_statuses() {
    let dir = scratch("gate-errors");
    for (name, flags) in [
        ("plain", vec![]),
        (
            "san",
            vec![
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
        ),
        (
            "tsan",
            vec!["-g", "-fsanitize=thread", "-fno-sanitize-recover=all"],
        ),
    ] {
        let binary = dir.0.join(name);
        compile("activity_gate_errors.c", &binary, &flags);
        for guard in [
            "init-memory",
            "init-again",
            "slow-lock",
            "claim-wait",
            "physical-unlock",
            "promote-lock",
            "signal",
            "handoff-lock",
            "broadcast",
        ] {
            run(
                &binary,
                &[guard],
                if guard.starts_with("init-") { 71 } else { 86 },
            );
        }
    }
}
