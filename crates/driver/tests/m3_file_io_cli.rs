#![cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
#[path = "../../../sdk/io/tests/package.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
mod platform;
#[path = "support/target.rs"]
mod target;
use std::process::{Command, Stdio};

#[test]
fn dynamic_file_bytes_and_owner_settlement_agree_on_jit_aot_and_sanitized_c() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-file-cli-{}", std::process::id()));
    let app = fixture::create(&root);
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../sdk/io/tests/dynamic.rinzu"),
    )
    .unwrap();
    for action in ["check", "run", "verify-aot", "build"] {
        let output = platform::bounded_tool(
            Command::new(frozen_cli::path())
                .arg(action)
                .arg(&app)
                .current_dir(&app),
        );
        platform::success(&output);
        if action == "run" {
            assert_eq!(output.stdout, b"io-dynamic-ok\n");
        }
    }
    let binary = target::build_directory(&app).join("io_source_app");
    let sanitized = platform::sanitize(&binary);
    let output = platform::bounded(
        Command::new(&sanitized)
            .current_dir(&app)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    );
    platform::success(&output);
    assert_eq!(output.stdout, b"io-dynamic-ok\n");
    #[cfg(target_os = "macos")]
    platform::sign(&binary, &root);
    let output = platform::bounded(Command::new(&binary).current_dir(&app));
    platform::success(&output);
    assert_eq!(output.stdout, b"io-dynamic-ok\n");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_file_read_cancellation_allows_owner_close_on_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-cancel-cli-{}", std::process::id()));
    let app = fixture::create(&root);
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../sdk/io/tests/cancel.rinzu"),
    )
    .unwrap();
    let output = platform::bounded_tool(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .current_dir(&app),
    );
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"io-cancel-settled\n", "{output:?}");
    let output = platform::bounded_tool(
        Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .current_dir(&app),
    );
    platform::success(&output);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_active_pipe_read_drains_before_owner_close_on_jit_aot_and_gc() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-active-cancel-{}", std::process::id()));
    let app = fixture::create(&root);
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../sdk/io/tests/cancel-active.rinzu"),
    )
    .unwrap();
    platform::success(&platform::bounded_tool(
        Command::new(frozen_cli::path()).arg("build").arg(&app),
    ));
    let binary = target::build_directory(&app).join("io_source_app");
    let sanitized = platform::sanitize(&binary);
    for stress in ["0", "1"] {
        for mut command in [
            {
                let mut command = Command::new(frozen_cli::path());
                command.arg("run").arg(&app);
                command
            },
            Command::new(&binary),
            Command::new(&sanitized),
        ] {
            // bounded_tool retains Child.stdin while polling. The writer stays
            // open without supplying bytes, so a successful read cannot race cancel.
            let output = platform::bounded_tool(
                command
                    .current_dir(&app)
                    .stdin(Stdio::piped())
                    .env("RINZU_GC_STRESS", stress)
                    .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
                    .env("UBSAN_OPTIONS", "halt_on_error=1"),
            );
            assert_eq!(output.status.code(), Some(1), "stress={stress}: {output:?}");
            assert_eq!(output.stdout, b"active-io-cancel-settled\n", "{output:?}");
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            assert!(
                diagnostic
                    .lines()
                    .any(|line| line == "RINZU_FAILURE=cancelled"),
                "stress={stress}: {output:?}"
            );
            assert!(
                !diagnostic.contains("ERROR: AddressSanitizer"),
                "{output:?}"
            );
            assert!(!diagnostic.contains("runtime error:"), "{output:?}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn large_binary_copy_saves_after_close_and_preserves_destination_on_rename_failure() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-copy-cli-{}", std::process::id()));
    let app = fixture::create(&root);
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../tests/fixtures/programs/file-copy/src/main.rinzu"),
    )
    .unwrap();
    let block: Vec<u8> = (0..65536).map(|i| (i % 256) as u8).collect();
    let input = block.repeat(128);
    std::fs::write(app.join("input.bin"), &input).unwrap();
    for action in ["run", "verify-aot", "build"] {
        let expected = if action == "verify-aot" {
            block.repeat(4)
        } else {
            input.clone()
        };
        std::fs::write(app.join("input.bin"), &expected).unwrap();
        let output = platform::bounded_tool(
            Command::new(frozen_cli::path())
                .arg(action)
                .arg(&app)
                .current_dir(&app),
        );
        platform::success(&output);
        if action == "run" {
            assert_eq!(output.stdout, b"8388608\n");
        }
        if action != "build" {
            assert!(
                std::fs::read(app.join("output.bin")).unwrap() == expected,
                "copy content mismatch for {action}"
            );
            assert!(!app.join("output.bin.tmp").exists());
        }
    }
    let binary = target::build_directory(&app).join("io_source_app");
    let output = platform::bounded(Command::new(&binary).current_dir(&app));
    platform::success(&output);
    assert_eq!(output.stdout, b"8388608\n");
    assert!(
        std::fs::read(app.join("output.bin")).unwrap() == input,
        "standalone copy content mismatch"
    );
    std::fs::remove_file(app.join("output.bin")).unwrap();
    std::fs::create_dir(app.join("output.bin")).unwrap();
    std::fs::write(app.join("output.bin/keep"), b"old").unwrap();
    let output = platform::bounded(Command::new(&binary).current_dir(&app));
    platform::success(&output);
    assert!(
        !output.stdout.is_empty() && output.stdout != b"8388608\n",
        "{output:?}"
    );
    assert_eq!(std::fs::read(app.join("output.bin/keep")).unwrap(), b"old");
    assert!(!app.join("output.bin.tmp").exists());
    std::fs::remove_dir_all(root).unwrap();
}
