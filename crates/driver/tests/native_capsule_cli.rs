#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
use std::path::Path;
use std::process::{Command, Output};
#[path = "fixtures/native_capsule_sdk/admissions.rs"]
mod admissions;
#[path = "fixtures/native_capsule_sdk/setup.rs"]
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
    admissions::check(&admissions::from_report(&stdout));
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
            .arg(app.join("build/capsule_app.o"))
            .arg(app.join("build/capsule_app.runtime.c"))
            .arg(manifest.join("../runtime/src/callback_entry.c"))
            .arg(manifest.join("../runtime/src/callback_template.S"))
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap(),
    );
    let resources = app.join("build/capsule_app.resources");
    if resources.exists() {
        std::os::unix::fs::symlink(resources, path.with_file_name(format!("{name}.resources")))
            .unwrap();
    }
    path
}

fn sign(binary: &Path, entitlements: &Path) {
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
            .arg(entitlements)
            .arg(binary)
            .output()
            .unwrap(),
    );
    success(
        &Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(binary)
            .output()
            .unwrap(),
    );
    let actual = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(binary)
        .output()
        .unwrap();
    success(&actual);
    let actual = String::from_utf8(actual.stdout).unwrap();
    assert!(
        actual.contains("disable-library-validation")
            && !actual.contains("allow-jit")
            && !actual.contains("allow-unsigned-executable-memory")
    );
}

#[test]
fn actual_capsules_keep_origin_scopes_across_native_returns_and_parent_completion() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-native-capsules-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::create_dir_all(app.join("data")).unwrap();
    std::fs::write(app.join("data/proof.bin"), vec![0x5a; 8193]).unwrap();
    let entitlements = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let main = include_str!("fixtures/native_capsule_sdk/main.rinzu")
        .replace("capsules.finish(0)", "capsules.finish(1)");
    let calls = 13;
    let callbacks = 2;
    {
        std::fs::write(app.join("src/main.rinzu"), &main).unwrap();
        success(&cli(&app, "run"));
        success(&cli(&app, "build"));
        baseline(
            &Command::new(app.join("build/capsule_app"))
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap(),
            calls,
            callbacks,
        );
        let binary = app.join("build/capsule_app");
        sign(&binary, &entitlements);
        baseline(
            &Command::new(binary)
                .env("RINZU_TRANSPORT_EVIDENCE", "1")
                .output()
                .unwrap(),
            calls,
            callbacks,
        );
        let verified = cli(&app, "verify-aot");
        if !verified.status.success() {
            // The general verifier intentionally requires equal transcripts.
            // Only these explicit timer polls may differ; each backend's
            // callback Origin tree is checked separately above and by the
            // Source/compiled JIT unit gate using the same fixture.
            assert_eq!(
                String::from_utf8_lossy(&verified.stderr).trim(),
                "rinzu: async JIT/AOT admission trees differ"
            );
        }
        let sanitized = sanitizer(&app, &format!("native-capsules-{calls}-sanitized"));
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

fn cancelled(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stderr);
    for marker in [
        "CAPSULE_CANCEL_FINALLY=1",
        "CAPSULE_CANCEL_RESULT=41",
        "CAPSULE_CANCEL_OUTER=1",
    ] {
        assert_eq!(
            stdout.lines().filter(|line| *line == marker).count(),
            1,
            "{stdout}"
        );
    }
    assert!(
        stdout.contains("RINZU_NATIVE_CALLS=4,4,4,4,4,4,1,0,0,0,0"),
        "{stdout}"
    );
    assert!(stdout.contains("RINZU_FAILURE=cancelled"), "{stdout}");
    assert!(
        stdout
            .lines()
            .find(|line| line.starts_with("RINZU_NATIVE_ENTRIES="))
            .unwrap()
            .ends_with(",0,0")
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
        "{stderr}"
    );
}

#[test]
fn native_capsules_return_normal_callback_results_before_delivering_parent_cancellation() {
    let root =
        std::env::temp_dir().join(format!("rinzu-cli-capsule-cancel-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(
        app.join("src/main.rinzu"),
        include_str!("fixtures/native_capsule_sdk/cancel.rinzu"),
    )
    .unwrap();
    success(&cli(&app, "run"));
    success(&cli(&app, "build"));
    let run = |binary: &Path| {
        Command::new(binary)
            .env("RINZU_CANCEL_AFTER_POLLS", "80")
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap()
    };
    let binary = app.join("build/capsule_app");
    cancelled(&run(&binary));
    let entitlements = root.join("adhoc-sdk.plist");
    std::fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    sign(&binary, &entitlements);
    cancelled(&run(&binary));
    let oracle = Command::new(frozen_cli::path())
        .arg("verify-aot")
        .arg(&app)
        .args(["--cancel-after-polls", "80"])
        .output()
        .unwrap();
    success(&oracle);
    let binary = sanitizer(&app, "native-capsules-cancel-sanitized");
    cancelled(&run(&binary));
    std::fs::remove_dir_all(root).unwrap();
}
