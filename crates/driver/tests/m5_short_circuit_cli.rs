#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::process::Command;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[test]
fn short_circuit_skips_traps_and_unstarted_futures_and_keeps_gc_roots() {
    let directory =
        std::env::temp_dir().join(format!("rinzu-short-circuit-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    for name in ["sync", "async"] {
        let source = directory.join(format!("{name}.rinzu"));
        std::fs::copy(
            format!(
                "{}/tests/fixtures/m5-short-circuit/{name}.rinzu",
                env!("CARGO_MANIFEST_DIR")
            ),
            &source,
        )
        .unwrap();
        for mode in ["run", "build"] {
            let mut command = Command::new(frozen_cli::path());
            command
                .arg(mode)
                .arg(&source)
                .env("RINZU_CACHE_DIR", directory.join("cache"))
                .env_remove("RINZU_TOOLCHAIN_ROOT");
            if mode == "build" {
                command.arg("--output").arg(directory.join(name));
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{name}/{mode}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if mode == "run" {
                assert!(
                    String::from_utf8_lossy(&output.stdout)
                        .contains(&format!("SHORT-CIRCUIT:{name}:"))
                );
            }
        }
        let output = Command::new(directory.join(name)).output().unwrap();
        assert!(
            output.status.success(),
            "{name}/aot: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&format!("SHORT-CIRCUIT:{name}:"))
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("failed:"));
    }
    std::fs::remove_dir_all(directory).unwrap();
}
