#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn c_jit_local_liveness_preserves_cfg_error_loops_cleanup_and_worker_results() {
    use std::{fs, path::Path, process::Command};
    let root = std::env::temp_dir().join(format!("rinzu-local-roots-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dead_local_roots.rinzu");
    for stress in ["0", "1"] {
        let report = root.join(format!("resources-{stress}.jsonl"));
        let result = Command::new(frozen_cli::path())
            .arg("run")
            .arg(&source)
            .arg("--resource-report")
            .arg(&report)
            .args(["--resource-interval-ms", "0"])
            .env("RINZU_GC_STRESS", stress)
            .env("RINZU_ERROR_TRACE", "1")
            .output()
            .unwrap();
        assert!(result.status.success(), "stress={stress}: {result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("compiled JIT continuation runtime")
        );
        assert!(
            String::from_utf8_lossy(&result.stdout)
                .contains("C06-local:cfg:overwrite:error:loop:finally:alias:gc:worker:OK")
        );
        let rows = fs::read_to_string(report).unwrap();
        let last: serde_json::Value = serde_json::from_str(rows.lines().last().unwrap()).unwrap();
        for category in last["categories"].as_array().unwrap() {
            if !matches!(
                category["id"].as_str(),
                Some("rss" | "virtual" | "footprint")
            ) {
                assert_eq!(category["live"].as_u64().unwrap_or(0), 0, "{category}");
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}
