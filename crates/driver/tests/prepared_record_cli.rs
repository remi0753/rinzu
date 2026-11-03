#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::path::Path;
use std::process::{Command, Output};

#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cli(app: &Path, command: &str) -> Output {
    Command::new(frozen_cli::path())
        .arg(command)
        .arg(app)
        .output()
        .unwrap()
}

#[test]
fn prepared_wide_and_generic_records_keep_callback_worker_and_snapshot_contracts() {
    let root = std::env::temp_dir().join(format!("rinzu-prepared-record-{}", std::process::id()));
    std::fs::create_dir_all(root.join("app/src")).unwrap();
    std::fs::create_dir_all(root.join("prepared/src")).unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"prepared_record_app\"\nlanguage=\"0.2\"\n[dependencies]\nprepared={path=\"../prepared\"}\n").unwrap();
    std::fs::write(root.join("prepared/rinzu.toml"), "[package]\nname=\"prepared\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    for (name, text) in [
        (
            "prepared/src/api.rinzu",
            include_str!("fixtures/prepared_record_sdk/api.rinzu"),
        ),
        (
            "prepared/api.h",
            include_str!("fixtures/prepared_record_sdk/api.h"),
        ),
        (
            "prepared/api.c",
            include_str!("fixtures/prepared_record_sdk/api.c"),
        ),
        (
            "app/src/main.rinzu",
            include_str!("fixtures/prepared_record_sdk/main.rinzu"),
        ),
    ] {
        std::fs::write(root.join(name), text).unwrap();
    }
    let app = root.join("app");
    success(&cli(&app, "build"));
    let runtime = std::fs::read_to_string(app.join("build/prepared_record_app.runtime.c")).unwrap();
    assert!(runtime.contains("rinzu_native_receive_record_prepared(\n"));
    assert!(runtime.contains("data->schema != &rinzu_program_schema"));
    assert_eq!(
        runtime
            .matches("static inline int rinzu_native_receive_record_")
            .count(),
        1
    );
    for stress in ["0", "1"] {
        let output = Command::new(app.join("build/prepared_record_app"))
            .env("RINZU_GC_STRESS", stress)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        success(&output);
        let report = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(report.contains("RINZU_FAILURE=none"), "{report}");
        let counts: Vec<usize> = report
            .lines()
            .find_map(|line| line.strip_prefix("RINZU_NATIVE_CALLS="))
            .unwrap()
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        assert_eq!(&counts[..6], &[54; 6], "{report}");
        assert_eq!(&counts[8..], &[0, 0, 0], "{report}");
    }
    // Source/JIT is a separate semantic reference. Its runtime-image ledger
    // need not have the same scope as the standalone AOT image.
    let source = Command::new(frozen_cli::path())
        .arg("run")
        .arg(&app)
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    success(&source);
    std::fs::remove_dir_all(root).unwrap();
}
