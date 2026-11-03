#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::Command;

#[test]
fn origin_stores_preserve_errors_reawait_finally_and_worker_contexts() {
    let root = std::env::temp_dir().join(format!("rinzu-origin-cli-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for name in ["pure", "native", "worker"] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/task_error_origin_{name}.rinzu"));
        let binary = root.join(name);
        let build = Command::new(frozen_cli::path())
            .arg("build")
            .arg(&fixture)
            .arg("--output")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        for stress in ["0", "1"] {
            let run = Command::new(&binary)
                .env("RINZU_GC_STRESS", stress)
                .env("RINZU_ERROR_TRACE", "1")
                .output()
                .unwrap();
            assert!(
                run.status.success(),
                "{}",
                String::from_utf8_lossy(&run.stderr)
            );
            if name == "native" {
                assert!(
                    String::from_utf8_lossy(&run.stdout)
                        .contains("C39:origin:future-alias:typed-Error:cleanup:OK\n")
                );
            }
            let reports = String::from_utf8_lossy(&run.stderr);
            let gc = reports
                .lines()
                .find_map(|line| line.strip_prefix("RINZU_FUTURE_GC="))
                .unwrap();
            let counts: Vec<usize> = gc.split(',').map(|s| s.parse().unwrap()).collect();
            assert!(counts[1] > 0 && counts[2] == 0, "{reports}");
            let equivalent = Command::new(frozen_cli::path())
                .arg("verify-aot")
                .arg(&fixture)
                .env("RINZU_GC_STRESS", stress)
                .env("RINZU_ERROR_TRACE", "1")
                .output()
                .unwrap();
            assert!(
                equivalent.status.success(),
                "{}",
                String::from_utf8_lossy(&equivalent.stderr)
            );
            let text = String::from_utf8_lossy(&equivalent.stdout);
            assert!(
                text.contains("outcome=success") && text.contains("resources=baseline"),
                "{text}"
            );
            println!("C39:{name}:gc{stress}:{gc}:equivalent");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
