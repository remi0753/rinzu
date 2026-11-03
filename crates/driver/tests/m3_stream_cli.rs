#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
mod platform;
use std::process::Command;
#[test]
fn c_task_cap_rejects_before_body_and_settles_the_admitted_children() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-stream-cap-{}", std::process::id()));
    let app = root.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("rinzu.toml"),
        "[package]\nname=\"stream_cap\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../sdk/stream/tests/admission.rinzu"),
    )
    .unwrap();
    let output = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .current_dir(&app),
    );
    platform::success(&output);
    assert_eq!(output.stdout, b"4\n");
    let output = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("build")
            .arg(&app)
            .current_dir(&app),
    );
    platform::success(&output);
    let binary = app.join("build/stream_cap");
    let runtime = binary.with_extension("runtime.c");
    let source = std::fs::read_to_string(&runtime).unwrap();
    // Private conformance profile, same as the Rust unit's limit=3. The
    // production default grows with available memory; only this private profile is bounded.
    let before = "rinzu_async_init(&async.runtime);";
    assert!(source.contains(before));
    std::fs::write(
        &runtime,
        source.replace(
            before,
            "rinzu_async_init(&async.runtime); async.runtime.main_task_limit=3;",
        ),
    )
    .unwrap();
    let sanitized = platform::sanitize(&binary);
    let report = root.join("cap.jsonl");
    let output = platform::bounded(
        Command::new(sanitized)
            .current_dir(&app)
            .env("RINZU_RESOURCE_REPORT", &report)
            .env("RINZU_RESOURCE_INTERVAL_MS", "0")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    );
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(output.stdout, b"3\n");
    let text = std::fs::read_to_string(report).unwrap();
    let end: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    for category in end["categories"].as_array().unwrap() {
        if matches!(
            category["id"].as_str().unwrap(),
            "task" | "future" | "pending_operation" | "managed" | "managed_objects"
        ) {
            assert_eq!(category["live"].as_u64(), Some(0), "{category}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn bounded_pull_eof_close_and_future_aliases_agree_on_jit_aot_and_sanitized_c() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-stream-basic-{}", std::process::id()));
    let app = root.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("rinzu.toml"),
        "[package]\nname=\"stream_probe\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../sdk/stream/tests/basic.rinzu"),
    )
    .unwrap();
    std::fs::write(
        app.join("input.bin"),
        (0..8192).map(|i| i as u8).collect::<Vec<_>>(),
    )
    .unwrap();
    for action in ["check", "run", "verify-aot", "build"] {
        let output = platform::bounded(
            Command::new(frozen_cli::path())
                .arg(action)
                .arg(&app)
                .current_dir(&app),
        );
        platform::success(&output);
        if action == "run" {
            assert_eq!(output.stdout, b"stream-basic-ok\n");
        }
        if action == "verify-aot" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("outcome=success"));
        }
    }
    let binary = app.join("build/stream_probe");
    let sanitized = platform::sanitize(&binary);
    let output = platform::bounded(
        Command::new(sanitized)
            .current_dir(&app)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    );
    platform::success(&output);
    assert_eq!(output.stdout, b"stream-basic-ok\n");
    platform::sign(&binary, &root);
    let output = platform::bounded(Command::new(&binary).current_dir(&app));
    platform::success(&output);
    assert_eq!(output.stdout, b"stream-basic-ok\n");
    std::fs::remove_dir_all(root).unwrap();
}
