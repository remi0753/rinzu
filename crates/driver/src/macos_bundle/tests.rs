use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-app-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config() -> config::Config {
    config::Config::parse(br#"{"schema":"rinzu.macos-app-v1","identifier":"dev.rinzu.test","name":"Test App","resources":["assets"]}"#).unwrap()
}

#[test]
fn metadata_rejects_malformed_identity_version_paths_and_unknown_fields() {
    let good = serde_json::to_value(config()).unwrap();
    for (key, value) in [
        ("identifier", serde_json::json!("../app")),
        ("identifier", serde_json::json!("dev..app")),
        ("name", serde_json::json!("../../app")),
        ("name", serde_json::json!("\u{0000}")),
        ("version", serde_json::json!("0.1")),
        ("build", serde_json::json!("0")),
        ("minimumSystemVersion", serde_json::json!("10.15")),
        ("icon", serde_json::json!("/tmp/icon.png")),
        ("resources", serde_json::json!(["assets/../secret"])),
        ("resources", serde_json::json!(["a/./b"])),
        ("unknown", serde_json::json!(true)),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert!(
            config::Config::parse(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "{bad}"
        );
    }
    let mut config = config();
    config.name = "日本語 & <App>".into();
    assert!(
        config
            .plist("test_app")
            .contains("日本語 &amp; &lt;App&gt;")
    );
}

#[test]
fn resource_snapshot_detects_edits_new_files_and_symlinks() {
    let fixture = Fixture::new();
    let root = fixture.0.canonicalize().unwrap();
    fs::create_dir(root.join("assets")).unwrap();
    fs::write(root.join("assets/日本語.txt"), "hello 😀").unwrap();
    let config = config();
    let bytes = serde_json::to_vec(&config).unwrap();
    fs::write(root.join("macos-app.json"), &bytes).unwrap();
    let snapshot =
        snapshot::Snapshot::capture(&root, Path::new("macos-app.json"), &bytes, &config).unwrap();
    snapshot
        .verify(&root, Path::new("macos-app.json"), &bytes, &config)
        .unwrap();
    fs::write(root.join("assets/日本語.txt"), "changed").unwrap();
    assert!(
        snapshot
            .verify(&root, Path::new("macos-app.json"), &bytes, &config)
            .is_err()
    );
    fs::write(root.join("assets/日本語.txt"), "hello 😀").unwrap();
    fs::write(root.join("assets/new.txt"), "new").unwrap();
    assert!(
        snapshot
            .verify(&root, Path::new("macos-app.json"), &bytes, &config)
            .is_err()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("../macos-app.json", root.join("assets/link")).unwrap();
        assert!(
            snapshot::Snapshot::capture(&root, Path::new("macos-app.json"), &bytes, &config)
                .unwrap_err()
                .contains("symlink")
        );
    }
}

#[cfg(unix)]
#[test]
fn non_regular_config_or_icon_is_rejected_without_waiting_for_a_writer() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let root = fixture.0.canonicalize().unwrap();
    let path = root.join("input");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a live NUL-terminated path inside this test's private directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(
        snapshot::read(&root, Path::new("input"), 65536)
            .unwrap_err()
            .contains("regular file")
    );
}

#[test]
fn app_layout_keeps_code_and_data_inside_contents() {
    assert_eq!(
        Layout::MacosApp.file("sdk-123/source-123.dylib").unwrap(),
        "Frameworks/source-123.dylib"
    );
    assert_eq!(
        Layout::MacosApp.file("sdk-123/source-package.h").unwrap(),
        "Resources/SourcePackages/sdk-123/source-package.h"
    );
    assert!(Layout::MacosApp.file("../../outside").is_err());
    assert!(
        Layout::Sidecars
            .file("sdk-123/source-123.dylib")
            .unwrap()
            .starts_with("sdk-123/")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn png_icons_round_trip_through_validated_icns_and_empty_containers_fail() {
    let fixture = Fixture::new();
    let png = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/programs/macos-charts/assets/AppIcon.png"),
    )
    .unwrap();
    let first = fixture.0.join("png");
    let first_resources = first.join("Resources");
    fs::create_dir_all(&first_resources).unwrap();
    icon::generate(&png, Path::new("icon.png"), &first, &first_resources).unwrap();
    let decoded = fs::read(first.join("validated-icon.png")).unwrap();
    assert_eq!(&decoded[16..24], &png[16..24]);
    let icns = fs::read(first_resources.join("Icon.icns")).unwrap();
    let second = fixture.0.join("icns");
    let second_resources = second.join("Resources");
    fs::create_dir_all(&second_resources).unwrap();
    icon::generate(&icns, Path::new("icon.icns"), &second, &second_resources).unwrap();
    assert_eq!(fs::read(second_resources.join("Icon.icns")).unwrap(), icns);
    let invalid = fixture.0.join("invalid");
    let invalid_resources = invalid.join("Resources");
    fs::create_dir_all(&invalid_resources).unwrap();
    let empty = [b"icns".as_slice(), &8u32.to_be_bytes()].concat();
    assert!(
        icon::generate(
            &empty,
            Path::new("empty.icns"),
            &invalid,
            &invalid_resources
        )
        .is_err()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn publication_refuses_existing_outputs_and_is_all_or_nothing() {
    let fixture = Fixture::new();
    let source = fixture.0.join("Prepared.app");
    let destination = fixture.0.join("Existing.app");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("new"), "new").unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep"), "keep").unwrap();
    assert!(publish(&source, &destination).is_err());
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"keep");
    let destination = fixture.0.join("New.app");
    publish(&source, &destination).unwrap();
    assert_eq!(fs::read(destination.join("new")).unwrap(), b"new");
    assert!(!source.exists());
}
