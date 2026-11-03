#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../../sdk/io/tests/package.rs"]
mod fixture;
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
mod platform;
use std::process::Command;

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
        let output = platform::bounded(
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
    let binary = app.join("build/io_source_app");
    let sanitized = platform::sanitize(&binary);
    let output = platform::bounded(
        Command::new(&sanitized)
            .current_dir(&app)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1"),
    );
    platform::success(&output);
    assert_eq!(output.stdout, b"io-dynamic-ok\n");
    platform::sign(&binary, &root);
    let output = platform::bounded(Command::new(&binary).current_dir(&app));
    platform::success(&output);
    assert_eq!(output.stdout, b"io-dynamic-ok\n");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_file_child_physically_drains_before_owner_close_on_jit_and_aot() {
    let root = std::env::temp_dir().join(format!("rinzu-m3-cancel-cli-{}", std::process::id()));
    let app = fixture::create(&root);
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("../../../sdk/io/tests/cancel.rinzu"),
    )
    .unwrap();
    let output = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&app)
            .current_dir(&app),
    );
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"io-cancel-settled\n", "{output:?}");
    let output = platform::bounded(
        Command::new(frozen_cli::path())
            .arg("verify-aot")
            .arg(&app)
            .current_dir(&app),
    );
    platform::success(&output);
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
        let output = platform::bounded(
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
    let output = platform::bounded(Command::new(app.join("build/io_source_app")).current_dir(&app));
    platform::success(&output);
    assert_eq!(output.stdout, b"8388608\n");
    assert!(
        std::fs::read(app.join("output.bin")).unwrap() == input,
        "standalone copy content mismatch"
    );
    std::fs::remove_file(app.join("output.bin")).unwrap();
    std::fs::create_dir(app.join("output.bin")).unwrap();
    std::fs::write(app.join("output.bin/keep"), b"old").unwrap();
    let output = platform::bounded(Command::new(app.join("build/io_source_app")).current_dir(&app));
    platform::success(&output);
    assert!(
        !output.stdout.is_empty() && output.stdout != b"8388608\n",
        "{output:?}"
    );
    assert_eq!(std::fs::read(app.join("output.bin/keep")).unwrap(), b"old");
    assert!(!app.join("output.bin.tmp").exists());
    std::fs::remove_dir_all(root).unwrap();
}
