#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/scratch.rs"]
mod scratch;

use std::path::Path;
use std::process::Command;

#[test]
fn observer_timeout_and_population_boundaries_match_jit_aot_and_gc() {
    let scratch = scratch::Scratch::new("rinzu-future-observation-contract");
    let artifacts = scratch.artifacts();
    std::fs::create_dir_all(&artifacts).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../codegen/tests/fixtures");
    for (fixture, expected) in [
        (
            "future-timeout-observation",
            b"true\nfalse\nheld failure\n".as_slice(),
        ),
        ("future-population-boundary", b"17\n1\n".as_slice()),
    ] {
        let source = scratch.path().join(format!("{fixture}.rinzu"));
        std::fs::copy(fixtures.join(format!("{fixture}.rinzu")), &source).unwrap();
        for stress in ["0", "1"] {
            let run = Command::new(frozen_cli::path())
                .arg("run")
                .arg(&source)
                .env("RINZU_GC_STRESS", stress)
                .output()
                .unwrap();
            assert!(
                run.status.success(),
                "{fixture} JIT stress={stress}: {run:?}"
            );
            assert_eq!(run.stdout, expected, "{fixture} JIT stress={stress}");
            let verified = Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&source)
                .env("RINZU_GC_STRESS", stress)
                .output()
                .unwrap();
            assert!(
                verified.status.success(),
                "{fixture} equivalence stress={stress}: {verified:?}"
            );
            assert!(
                String::from_utf8_lossy(&verified.stdout).contains("outcome=success"),
                "{verified:?}"
            );
        }
        let binary = artifacts.join(fixture);
        let build = Command::new(frozen_cli::path())
            .arg("build")
            .arg(&source)
            .arg("--output")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(build.status.success(), "{fixture}: {build:?}");
        let aot = Command::new(&binary)
            .env("RINZU_GC_STRESS", "1")
            .output()
            .unwrap();
        assert!(aot.status.success(), "{fixture}: {aot:?}");
        assert_eq!(aot.stdout, expected, "{fixture} standalone AOT");
    }
}
