#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "fixtures/frozen_cli.rs"]
mod frozen_cli;
#[path = "support/source_platform.rs"]
#[allow(dead_code)]
mod platform;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let root = std::env::temp_dir().join(format!(
                "rinzu-macos-bundle-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            match fs::create_dir(&root) {
                Ok(()) => return Self(root),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create {}: {error}", root.display()),
            }
        }
    }
    fn cli(&self, args: &[&std::ffi::OsStr]) -> Output {
        platform::bounded_tool(
            Command::new(frozen_cli::path())
                .args(args)
                .current_dir(&self.0)
                .env("RINZU_CACHE_DIR", self.0.join("cache")),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn copy(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        if [".rinzu", "build"]
            .iter()
            .any(|name| entry.file_name() == *name)
        {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn rejected(output: Output, marker: &str) {
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(marker),
        "{output:?}"
    );
}
fn baseline(path: &Path) {
    let record: serde_json::Value =
        serde_json::from_str(fs::read_to_string(path).unwrap().lines().last().unwrap()).unwrap();
    assert_eq!(record["phase"], "end");
    let categories = record["categories"].as_array().unwrap();
    for name in [
        "managed",
        "task",
        "future",
        "pending_operation",
        "transport",
        "native_calls",
    ] {
        assert_eq!(
            categories.iter().find(|c| c["id"] == name).unwrap()["live"],
            0,
            "{record}"
        );
    }
}

#[test]
fn signed_app_relocates_without_sources_preserves_native_receipts_and_rejects_tampering() {
    let f = Fixture::new();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let sources = f.0.join("packages");
    let sdk = sources.join("sdk/macos");
    copy(&repo.join("sdk/macos"), &sdk);
    let package = sdk.join("tests/bundle");
    fs::copy(
        repo.join("tests/fixtures/programs/macos-charts/assets/AppIcon.png"),
        package.join("assets/icon.png"),
    )
    .unwrap();
    let config_path = package.join("macos-app.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    config["icon"] = "assets/icon.png".into();
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    let report = f.0.join("jit.jsonl");
    let jit = platform::bounded_tool(
        Command::new(frozen_cli::path())
            .arg("run")
            .arg(&package)
            .args(["--resource-interval-ms", "0", "--resource-report"])
            .arg(&report)
            .env("RINZU_BUNDLE_DEV_ROOT", &package)
            .env("RINZU_CACHE_DIR", f.0.join("cache")),
    );
    platform::success(&jit);
    baseline(&report);
    let bundle = f.0.join("Initial 日本語.app");
    platform::success(&f.cli(&[
        "bundle".as_ref(),
        package.as_os_str(),
        "--output".as_ref(),
        bundle.as_os_str(),
    ]));
    assert!(bundle.join("Contents/Resources/Icon.icns").is_file());
    assert!(
        bundle
            .join("Contents/Resources/app/assets/日本語.txt")
            .is_file()
    );
    assert!(
        bundle
            .join("Contents/Resources/Runtime/data/probe.txt")
            .is_file()
    );
    for entry in fs::read_dir(bundle.join("Contents/Frameworks")).unwrap() {
        assert_eq!(entry.unwrap().path().extension().unwrap(), "dylib");
    }
    rejected(
        f.cli(&[
            "bundle".as_ref(),
            package.as_os_str(),
            "--output".as_ref(),
            bundle.as_os_str(),
        ]),
        "already exists",
    );
    // Exercise the ordinary AOT path as well as the new app layout.
    let bare = f.0.join("bare");
    platform::success(&f.cli(&[
        "build".as_ref(),
        package.as_os_str(),
        "--output".as_ref(),
        bare.as_os_str(),
    ]));
    let ordinary = platform::bounded_tool(
        Command::new(&bare)
            .current_dir(&f.0)
            .env("RINZU_BUNDLE_DEV_ROOT", &package)
            .env("RINZU_RESOURCE_REPORT", f.0.join("bare.jsonl"))
            .env("RINZU_RESOURCE_INTERVAL_MS", "0"),
    );
    platform::success(&ordinary);
    assert_eq!(jit.stdout, ordinary.stdout);
    baseline(&f.0.join("bare.jsonl"));
    let moved = f.0.join("Moved 東京.app");
    fs::rename(&bundle, &moved).unwrap();
    fs::remove_dir_all(&sources).unwrap();
    fs::remove_dir_all(f.0.join("cache")).unwrap();
    platform::success(&f.cli(&["bundle-verify".as_ref(), moved.as_os_str()]));
    let executable = moved.join("Contents/MacOS/macos_bundle_probe");
    let aot = platform::bounded_tool(
        Command::new(&executable)
            .current_dir(&f.0)
            .env("RINZU_BUNDLE_EXPECT_APP", "1")
            .env_remove("RINZU_TOOLCHAIN_ROOT")
            .env_remove("RINZU_CACHE_DIR")
            .env("RINZU_RESOURCE_REPORT", f.0.join("app.jsonl"))
            .env("RINZU_RESOURCE_INTERVAL_MS", "0"),
    );
    platform::success(&aot);
    assert_eq!(jit.stdout, aot.stdout);
    baseline(&f.0.join("app.jsonl"));
    let asset = moved.join("Contents/Resources/app/assets/日本語.txt");
    let bytes = fs::read(&asset).unwrap();
    fs::write(&asset, "tampered").unwrap();
    rejected(
        f.cli(&["bundle-verify".as_ref(), moved.as_os_str()]),
        "inventory changed",
    );
    fs::write(&asset, bytes).unwrap();
    let framework = fs::read_dir(moved.join("Contents/Frameworks"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = fs::read(&framework).unwrap();
    let mut changed = bytes.clone();
    changed.push(0);
    fs::write(&framework, changed).unwrap();
    rejected(
        f.cli(&["bundle-verify".as_ref(), moved.as_os_str()]),
        "inventory changed",
    );
    let failure =
        platform::bounded_tool(Command::new(&executable).env("RINZU_BUNDLE_EXPECT_APP", "1"));
    assert_eq!(failure.status.code(), Some(78), "{failure:?}");
    assert!(!failure.stdout.windows(7).any(|s| s == b"BUNDLE:"));
    fs::write(&framework, bytes).unwrap();
    platform::success(&f.cli(&["bundle-verify".as_ref(), moved.as_os_str()]));
}

#[test]
fn plain_apps_need_no_sdk_and_invalid_inputs_never_publish_output() {
    let f = Fixture::new();
    let package = f.0.join("plain");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("rinzu.toml"),
        "[package]\nname=\"plain_app\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    fs::write(package.join("src/main.rinzu"), "fn main() {}\n").unwrap();
    let path = package.join("macos-app.json");
    fs::write(
        &path,
        r#"{"schema":"rinzu.macos-app-v1","identifier":"dev.rinzu.plain","name":"Plain App"}"#,
    )
    .unwrap();
    let bundle = f.0.join("Plain.app");
    platform::success(&f.cli(&[
        "bundle".as_ref(),
        package.as_os_str(),
        "--output".as_ref(),
        bundle.as_os_str(),
    ]));
    platform::success(&f.cli(&["bundle-verify".as_ref(), bundle.as_os_str()]));
    platform::success(&platform::bounded(
        Command::new(bundle.join("Contents/MacOS/plain_app")).current_dir(&f.0),
    ));
    fs::write(&path, r#"{"schema":"rinzu.macos-app-v1","identifier":"dev.rinzu.plain","name":"Plain App","resources":["../outside"]}"#).unwrap();
    let invalid = f.0.join("Invalid.app");
    rejected(
        f.cli(&[
            "bundle".as_ref(),
            package.as_os_str(),
            "--output".as_ref(),
            invalid.as_os_str(),
        ]),
        "package-relative",
    );
    assert!(!invalid.exists());
    fs::write(&path, r#"{"schema":"rinzu.macos-app-v1","identifier":"dev.rinzu.plain","name":"Plain App","icon":"icon.png"}"#).unwrap();
    fs::write(package.join("icon.png"), b"invalid").unwrap();
    rejected(
        f.cli(&[
            "bundle".as_ref(),
            package.as_os_str(),
            "--output".as_ref(),
            invalid.as_os_str(),
        ]),
        "1024x1024",
    );
    assert!(!invalid.exists());
    assert!(!fs::read_dir(&f.0).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rinzu-app-")
    }));
}
