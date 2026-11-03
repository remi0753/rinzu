#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_thread_sdk/setup.rs"]
mod fixture;

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}
fn cli(app: &Path, command: &str) -> Output {
    Command::new(frozen_cli::path())
        .arg(command)
        .arg(app)
        .output()
        .unwrap()
}
fn baseline(output: &Output, calls: usize, callbacks: usize) {
    success(output);
    let stdout = String::from_utf8_lossy(&output.stderr);
    let values: Vec<usize> = stdout
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_NATIVE_CALLS="))
        .unwrap()
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(&values[..6], &[calls; 6], "{stdout}");
    assert_eq!(&values[6..], &[callbacks, 0, 0, 0, 0], "{stdout}");
    assert!(stdout.contains("RINZU_CALLBACK_RETURNS=0,0,0,0"));
    assert!(stdout.contains("RINZU_FAILURE=none"));
    assert!(
        stdout
            .lines()
            .find(|line| line.starts_with("RINZU_NATIVE_ENTRIES="))
            .unwrap()
            .ends_with(",0,0")
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("ERROR: AddressSanitizer"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("runtime error:"));
}

fn sanitizer(app: &Path, name: &str) -> std::path::PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = app.join("build").join(name);
    success(
        &Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pthread",
                "-fsanitize=address,undefined",
                "-fno-omit-frame-pointer",
            ])
            .arg(app.join("build/thread_app.o"))
            .arg(app.join("build/thread_app.runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap(),
    );
    let resources = app.join("build/thread_app.resources");
    if resources.exists() {
        std::os::unix::fs::symlink(resources, path.with_file_name(format!("{name}.resources")))
            .unwrap();
    }
    path
}

#[test]
fn actual_sdk_keeps_main_worker_threads_nested_gc_and_explicit_native_lifetimes() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-native-threads-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::create_dir_all(app.join("data")).unwrap();
    std::fs::write(app.join("data/proof.bin"), vec![0x5a; 8193]).unwrap();
    let entitlements = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    for (main, calls, callbacks) in [
        (
            include_str!("fixtures/native_thread_sdk/main.rinzu"),
            426,
            126,
        ),
        (
            "import threads;fn main(){let normal=threads.exercise(1);let orphan=threads.orphan(1);if normal+orphan!=2||threads.live()!=1||threads.clear()!=1||threads.live()!=0{panic(\"wrong\");}}",
            50,
            14,
        ),
        (
            include_str!("fixtures/native_thread_sdk/blocked.rinzu"),
            10,
            1,
        ),
    ] {
        std::fs::write(app.join("src/main.rinzu"), main).unwrap();
        success(&cli(&app, "run"));
        success(&cli(&app, "build"));
        baseline(
            &Command::new(app.join("build/thread_app"))
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap(),
            calls,
            callbacks,
        );
        let binary = app.join("build/thread_app");
        success(
            &Command::new("codesign")
                .args([
                    "--force",
                    "--sign",
                    "-",
                    "--options",
                    "runtime",
                    "--entitlements",
                ])
                .arg(&entitlements)
                .arg(&binary)
                .output()
                .unwrap(),
        );
        success(
            &Command::new("codesign")
                .args(["--verify", "--strict"])
                .arg(&binary)
                .output()
                .unwrap(),
        );
        let actual = Command::new("codesign")
            .args(["-d", "--entitlements", "-"])
            .arg(&binary)
            .output()
            .unwrap();
        success(&actual);
        let actual = String::from_utf8(actual.stdout).unwrap();
        assert!(
            actual.contains("disable-library-validation")
                && !actual.contains("allow-jit")
                && !actual.contains("allow-unsigned-executable-memory")
        );
        baseline(
            &Command::new(binary)
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap(),
            calls,
            callbacks,
        );
        success(&cli(&app, "verify-aot"));
        let sanitized = sanitizer(&app, &format!("native-threads-{calls}-sanitized"));
        baseline(
            &Command::new(sanitized)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap(),
            calls,
            callbacks,
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
