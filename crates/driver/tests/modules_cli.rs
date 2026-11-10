#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use flate2::Compression;
use flate2::write::GzEncoder;
use rinzu_compiler::standard_library::sha256;

const SDK: &str = "[package]\nname = \"values\"\nlanguage = \"0.5\"\nkind = \"sdk\"\n";
const SOURCE: &str = "fn value() -> Int { return 42; } export value;";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-modules-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::write(
            root.join("app/rinzu.toml"),
            "[package]\nname = \"app\"\nlanguage = \"0.5\"\n",
        )
        .unwrap();
        fs::write(root.join("app/src/main.rinzu"), "fn main() {}").unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn cli(&self, args: &[&str]) -> Output {
        let path = std::env::join_paths(
            std::iter::once(self.path("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_rinzu"))
            .current_dir(self.path("app"))
            .env("RINZU_CACHE_DIR", self.path("cache"))
            .env("RINZU_TEST_ARCHIVES", &self.0)
            .env("PATH", path)
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn archive(path: &Path, name: &str, manifest: &str, source: &str) -> String {
    let encoder = GzEncoder::new(fs::File::create(path).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    for (name, body) in [
        ("sdk/rinzu.toml".to_owned(), manifest),
        (format!("sdk/src/{name}.rinzu"), source),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        tar.append_data(&mut header, name, body.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap();
    sha256(&fs::read(path).unwrap())
}

fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn rejected(output: Output, expected: &str) {
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && error.contains(expected),
        "{error}"
    );
}

#[test]
fn pinned_local_archive_restores_and_repairs_with_sibling_path_dependency() {
    let fixture = Fixture::new();
    let path = fixture.path("values.tar.gz");
    let digest = archive(&path, "values", SDK, SOURCE);
    fs::create_dir_all(fixture.path("helper/src")).unwrap();
    fs::write(
        fixture.path("helper/rinzu.toml"),
        SDK.replace("values", "helper"),
    )
    .unwrap();
    fs::write(fixture.path("helper/src/helper.rinzu"), SOURCE).unwrap();
    fs::write(fixture.path("app/rinzu.toml"), concat!(
        "\u{feff}[package]\r\nname = \"app\"\r\nlanguage = \"0.5\"\r\n",
        "\r\n[dependencies] # keep this comment\r\nhelper = { path = \"../helper\" } # and this\r\n",
    )).unwrap();
    fs::write(fixture.path("app/src/main.rinzu"), "import values; import helper; fn main() { if values.value() != helper.value() { panic(\"different value\"); } }").unwrap();
    success(fixture.cli(&["modules", "add", "values", "../values.tar.gz"]));
    let lock_path = fixture.path("app/rinzu.modules.lock");
    let original = fs::read(&lock_path).unwrap();
    let manifest = fs::read_to_string(fixture.path("app/rinzu.toml")).unwrap();
    assert!(manifest.starts_with('\u{feff}'));
    assert!(manifest.contains("helper = { path = \"../helper\" } # and this\r\n"));
    assert!(manifest.contains("\"values\" = { archive = \"../values.tar.gz\" }\r\n"));

    let source = fixture.path(&format!(
        "app/modules/store/sha256/{digest}/package/src/values.rinzu"
    ));
    fs::write(&source, "fn value() -> Int { return 43; } export value;").unwrap();
    let error = rinzu_compiler::check_package(&fixture.path("app")).unwrap_err();
    assert!(
        error
            .iter()
            .any(|diagnostic| diagnostic.message.contains("verified inventory"))
    );
    success(fixture.cli(&["modules", "sync", "--offline"]));
    assert_eq!(fs::read_to_string(&source).unwrap(), SOURCE);

    fs::remove_dir_all(fixture.path("app/modules")).unwrap();
    fs::remove_file(&path).unwrap();
    success(fixture.cli(&["modules", "sync", "--offline"]));
    rinzu_compiler::check_package(&fixture.path("app")).unwrap();
    assert_eq!(fs::read(&lock_path).unwrap(), original);

    fs::remove_dir_all(fixture.path("app/modules")).unwrap();
    fs::remove_dir_all(fixture.path("cache")).unwrap();
    archive(
        &path,
        "values",
        SDK,
        "fn value() -> Int { return 0; } export value;",
    );
    rejected(fixture.cli(&["modules", "sync"]), "SHA-256 mismatch");
    assert_eq!(fs::read(&lock_path).unwrap(), original);
}

#[test]
fn published_diamond_fetches_shared_sdk_once_and_keeps_direct_visibility() {
    let fixture = Fixture::new();
    let values = archive(&fixture.path("values.tar.gz"), "values", SDK, SOURCE);
    for parent in ["left", "right"] {
        let manifest = format!(
            "{}\n[dependencies]\nvalues = {{ archive = \"https://archive.example/values.tar.gz\", integrity = \"sha256:{values}\" }}\n",
            SDK.replace("values", parent),
        );
        archive(
            &fixture.path(&format!("{parent}.tar.gz")),
            parent,
            &manifest,
            "import values; export values.value;",
        );
    }
    fs::create_dir(fixture.path("bin")).unwrap();
    let curl = fixture.path("bin/curl");
    fs::write(&curl, r#"#!/bin/sh
set -eu
for argument do url=$argument; done
case "$url" in
    https://archive.example/left.tar.gz|https://archive.example/right.tar.gz|https://archive.example/values.tar.gz) ;;
    *) exit 2 ;;
esac
file=${url##*/}
printf '%s\n' "$file" >> "$RINZU_TEST_ARCHIVES/requests"
cat "$RINZU_TEST_ARCHIVES/$file"
"#).unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    for parent in ["left", "right"] {
        success(fixture.cli(&[
            "modules",
            "add",
            parent,
            &format!("https://archive.example/{parent}.tar.gz"),
        ]));
    }
    let requests = fs::read_to_string(fixture.path("requests")).unwrap();
    assert_eq!(
        requests
            .lines()
            .filter(|line| *line == "values.tar.gz")
            .count(),
        1
    );
    fs::write(fixture.path("app/src/main.rinzu"), "import left; import right; fn main() { if left.value() != right.value() { panic(\"different value\"); } }").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&fixture.path("app")).unwrap();
    assert_eq!(artifact.sdk_packages.len(), 3);
    fs::write(
        fixture.path("app/src/main.rinzu"),
        "import values; fn main() {}",
    )
    .unwrap();
    assert!(rinzu_compiler::check_package(&fixture.path("app")).is_err());
    success(fixture.cli(&["modules", "sync", "--offline"]));
    assert_eq!(
        fs::read_to_string(fixture.path("requests")).unwrap(),
        requests
    );
}

#[test]
fn unsafe_archive_is_rejected_before_dependency_commit() {
    let fixture = Fixture::new();
    let encoder = GzEncoder::new(
        fs::File::create(fixture.path("unsafe.tar.gz")).unwrap(),
        Compression::fast(),
    );
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(1);
    header.set_mode(0o644);
    header.as_mut_bytes()[..9].copy_from_slice(b"../escape");
    header.set_cksum();
    tar.append(&header, &b"x"[..]).unwrap();
    tar.into_inner().unwrap().finish().unwrap();

    let manifest = fixture.path("app/rinzu.toml");
    let before = fs::read(&manifest).unwrap();
    rejected(
        fixture.cli(&["modules", "add", "values", "../unsafe.tar.gz"]),
        "unsafe path",
    );
    assert_eq!(fs::read(manifest).unwrap(), before);
    assert!(!fixture.path("app/rinzu.modules.lock").exists());
    assert!(!fixture.path("escape").exists());
}
