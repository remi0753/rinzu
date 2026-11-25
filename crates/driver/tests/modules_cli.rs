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
        self.command(Path::new(env!("CARGO_BIN_EXE_rinzu")))
            .args(args)
            .output()
            .unwrap()
    }

    fn command(&self, binary: &Path) -> Command {
        let path = std::env::join_paths(
            std::iter::once(self.path("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut command = Command::new(binary);
        command
            .current_dir(self.path("app"))
            .env("RINZU_CACHE_DIR", self.path("cache"))
            .env("RINZU_TEST_ARCHIVES", &self.0)
            .env("PATH", path)
            .env_remove("RINZU_TOOLCHAIN_ROOT");
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn archive(path: &Path, name: &str, manifest: &str, source: &str) -> String {
    archive_with_files(path, name, manifest, source, &[])
}

fn archive_with_files(
    path: &Path,
    name: &str,
    manifest: &str,
    source: &str,
    files: &[(&str, &str)],
) -> String {
    let encoder = GzEncoder::new(fs::File::create(path).unwrap(), Compression::fast());
    let mut tar = tar::Builder::new(encoder);
    for (name, body) in [
        ("sdk/rinzu.toml".to_owned(), manifest),
        (format!("sdk/src/{name}.rinzu"), source),
    ]
    .into_iter()
    .chain(
        files
            .iter()
            .map(|(path, body)| (format!("sdk/{path}"), *body)),
    ) {
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
fn incompatible_archive_preserves_the_manifest_and_lock() {
    let fixture = Fixture::new();
    let manifest = fixture.path("app/rinzu.toml");
    let mut original = fs::read_to_string(&manifest).unwrap();
    original = original.replace("0.5", "0.4");
    fs::write(&manifest, &original).unwrap();
    let archive_path = fixture.path("values.tar.gz");
    archive(&archive_path, "values", SDK, SOURCE);
    rejected(
        fixture.cli(&["modules", "add", "values", "../values.tar.gz"]),
        "project language 0.4 is below required language 0.5 (app -> values",
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);
    assert!(!fixture.path("app/rinzu.modules.lock").exists());

    let required = format!("{SDK}\n[requires]\ntoolchain = \"99.0.0\"\n");
    archive(&archive_path, "values", &required, SOURCE);
    original = original.replace("0.4", "0.5");
    fs::write(&manifest, &original).unwrap();
    rejected(
        fixture.cli(&["modules", "add", "values", "../values.tar.gz"]),
        "Rinzu 99.0.0 or later is required (app -> values)",
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);
    assert!(!fixture.path("app/rinzu.modules.lock").exists());
}

#[test]
fn relock_updates_language_without_reselecting_an_archive() {
    let fixture = Fixture::new();
    archive(
        &fixture.path("values.tar.gz"),
        "values",
        &SDK.replace("0.5", "0.4"),
        SOURCE,
    );
    success(fixture.cli(&["modules", "add", "values", "../values.tar.gz"]));
    let lock = fixture.path("app/rinzu.modules.lock");
    let before: serde_json::Value = serde_json::from_slice(&fs::read(&lock).unwrap()).unwrap();
    let manifest = fixture.path("app/rinzu.toml");
    let lower = fs::read_to_string(&manifest).unwrap().replace("0.5", "0.4");
    fs::write(&manifest, &lower).unwrap();
    fs::remove_file(fixture.path("values.tar.gz")).unwrap();
    rejected(
        fixture.cli(&["modules", "sync", "--offline"]),
        "does not match",
    );
    success(fixture.cli(&["modules", "relock"]));
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&lock).unwrap()).unwrap();
    assert_ne!(
        before["packages"][0]["declarations_sha256"],
        after["packages"][0]["declarations_sha256"]
    );
    assert_eq!(before["packages"][1], after["packages"][1]);
    success(fixture.cli(&["modules", "sync", "--offline"]));
    success(fixture.cli(&["check", "."]));

    fs::write(&manifest, lower.replace("values.tar.gz", "other.tar.gz")).unwrap();
    rejected(fixture.cli(&["modules", "relock"]), "does not match");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(lock).unwrap()).unwrap(),
        after
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn relocated_toolchain_resolves_official_imports_and_verifies_its_catalog() {
    let fixture = Fixture::new();
    success(fixture.cli(&["toolchain-bundle", "--output", "../toolchain"]));
    fs::rename(
        fixture.path("toolchain"),
        fixture.path("relocated toolchain"),
    )
    .unwrap();
    let installed = fixture.path("relocated toolchain/bin/rinzu");
    let run = |args: &[&str]| fixture.command(&installed).args(args).output().unwrap();
    success(run(&["doctor", "--sdk", "rinzu.codec"]));
    let entry = fixture.path("app/src/main.rinzu");
    let direct = "import rinzu.codec; fn main() { let document = rinzu.codec.Document(text: \"Rinzu\", selection: 0); }";
    fs::write(&entry, direct).unwrap();
    success(run(&["check", "."]));
    success(run(&["check", "src/main.rinzu"]));

    archive(
        &fixture.path("values.tar.gz"),
        "values",
        SDK,
        &format!("import rinzu.codec; {SOURCE}"),
    );
    success(run(&["modules", "add", "values", "../values.tar.gz"]));
    fs::write(&entry, format!("import values; {direct}")).unwrap();
    let checked = fixture
        .command(&installed)
        .env("RINZU_COMPILE_TIMINGS", "1")
        .args(["check", "."])
        .output()
        .unwrap();
    let timing = String::from_utf8_lossy(&checked.stderr);
    assert_eq!(
        timing
            .lines()
            .filter(|line| line.contains("\"detail\":\"rinzu.codec\"")
                && line.contains("\"phase\":\"frontend.package\""))
            .count(),
        1,
        "{timing}"
    );
    success(checked);

    let bundle = fixture.path("relocated toolchain/bundle.json");
    let expected = fs::read_to_string(&bundle).unwrap();
    let mut corrupt: serde_json::Value = serde_json::from_str(&expected).unwrap();
    corrupt["officialSdkCatalogSha256"] = serde_json::Value::String("0".repeat(64));
    fs::write(&bundle, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    rejected(run(&["check", "."]), "installed SDK catalog does not match");
    fs::write(&bundle, expected).unwrap();

    let codec = fixture.path("relocated toolchain/share/rinzu/sdk/rinzu.codec/src/codec.rinzu");
    let source = fs::read(&codec).unwrap();
    fs::write(&codec, "fn unrelated() {}").unwrap();
    rejected(run(&["check", "."]), "verified inventory");
    fs::write(&codec, source).unwrap();

    archive(
        &fixture.path("native_user.tar.gz"),
        "native_user",
        &SDK.replace("values", "native_user"),
        &format!("import rinzu.sqlite; {SOURCE}"),
    );
    success(run(&[
        "modules",
        "add",
        "native_user",
        "../native_user.tar.gz",
    ]));
    rejected(
        run(&["check", "."]),
        "published SDK cannot import toolchain package `rinzu.sqlite`",
    );
}

#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
#[test]
fn macos_sdk_resolves_only_from_an_explicit_dependency() {
    let fixture = Fixture::new();
    success(fixture.cli(&["toolchain-bundle", "--output", "../toolchain"]));
    let bundle = fixture.path("toolchain");
    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("bundle.json")).unwrap()).unwrap();
    let packages = metadata["officialSdkCatalog"]["packages"]
        .as_object()
        .unwrap();
    assert!(!packages.contains_key("rinzu.macos"));
    assert!(!bundle.join("share/rinzu/sdk/rinzu.macos").exists());

    let installed = bundle.join("bin/rinzu");
    let run = |args: &[&str]| fixture.command(&installed).args(args).output().unwrap();
    success(run(&["doctor", "--sdk", "rinzu.codec"]));
    rejected(
        run(&["doctor", "--sdk", "rinzu.macos"]),
        "unknown additional official SDK `rinzu.macos`",
    );
    fs::write(
        fixture.path("app/src/main.rinzu"),
        "import rinzu.macos; fn main() { let value = rinzu.macos.value(); }",
    )
    .unwrap();
    rejected(
        run(&["check", "."]),
        "SDK package `rinzu.macos` is not a declared dependency",
    );

    let manifest = fixture.path("app/rinzu.toml");
    let package = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!(
            r#"{package}
[dependencies]
"rinzu.macos" = {{ path = "../macos" }}
"#
        ),
    )
    .unwrap();
    fs::create_dir_all(fixture.path("macos/src")).unwrap();
    let sdk = SDK.replace("values", "rinzu.macos");
    let sdk_manifest = fixture.path("macos/rinzu.toml");
    fs::write(&sdk_manifest, &sdk).unwrap();
    fs::write(fixture.path("macos/src/macos.rinzu"), SOURCE).unwrap();
    success(run(&["check", "."]));

    #[cfg(target_os = "linux")]
    {
        fs::write(
            &sdk_manifest,
            format!(
                r#"{sdk}
[native]
frameworks = ["AppKit"]
"#
            ),
        )
        .unwrap();
        rejected(run(&["check", "."]), "package `rinzu.macos` requires macOS");
    }
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

fn timing_phase(output: &Output, name: &str, detail: &str) -> bool {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter_map(|line| line.strip_prefix("RINZU_COMPILE_TIMING="))
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .any(|event| event["phase"] == name && event["detail"] == detail)
}

#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
#[test]
fn native_archive_builds_from_pinned_sources_and_repairs_inventory_changes() {
    let fixture = Fixture::new();
    let manifest = format!(
        r#"{SDK}
[native]
sources = ["native/bridge.c", "native/detail.c"]
include-dirs = ["include"]
"#
    );
    let source = r#"
native {
    fn value() -> CInt symbol "archive_value";
}
export value;
"#;
    let native_source = "#include \"detail.h\"\nint archive_value(void) { return answer(); }\n";
    let detail = "int answer(void);\n";
    let digest = archive_with_files(
        &fixture.path("values.tar.gz"),
        "values",
        &manifest,
        source,
        &[
            ("native/bridge.c", native_source),
            ("native/detail.c", "int answer(void) { return 42; }\n"),
            ("include/detail.h", detail),
        ],
    );
    fs::write(
        fixture.path("app/src/main.rinzu"),
        r#"import values;
import rinzu.timer;
fn expected() -> CInt { return CInt(42); }
async fn main() {
    try {
        await rinzu.timer.sleep(1);
        if values.value() != expected() { panic("wrong native value"); }
    } catch errors { panic("native call failed"); }
}"#,
    )
    .unwrap();
    success(fixture.cli(&["modules", "add", "values", "../values.tar.gz"]));
    let run = || {
        fixture
            .command(Path::new(env!("CARGO_BIN_EXE_rinzu")))
            .args(["run", "."])
            .env("RINZU_COMPILE_TIMINGS", "1")
            .output()
            .unwrap()
    };
    let cold = run();
    assert!(timing_phase(&cold, "native.package.early.store", "values"));
    success(cold);
    let warm = run();
    assert!(
        timing_phase(&warm, "native.package.early.hit", "values"),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(!timing_phase(
        &warm,
        "native.package.link.provider",
        "values"
    ));
    assert!(!timing_phase(&warm, "native.contract.validate", "values"));
    assert!(String::from_utf8_lossy(&warm.stderr).contains("\"phase\":\"jit.frontend.cache.hit\""));
    success(warm);

    let root = fixture.path(&format!("app/modules/store/sha256/{digest}/package"));
    for (name, content) in [
        ("native/bridge.c", native_source),
        ("include/detail.h", detail),
    ] {
        let input = root.join(name);
        fs::write(&input, "changed").unwrap();
        rejected(fixture.cli(&["run", "."]), "verified inventory");
        success(fixture.cli(&["modules", "sync", "--offline"]));
        assert_eq!(fs::read_to_string(&input).unwrap(), content);
    }

    let shadow_header = root.join("native/detail.h");
    fs::write(&shadow_header, "#define ANSWER 0\n").unwrap();
    rejected(
        fixture.cli(&["run", "."]),
        "unexpected input in SDK archive inventory",
    );
    success(fixture.cli(&["modules", "sync", "--offline"]));
    assert!(!shadow_header.exists());
    success(fixture.cli(&["run", "."]));
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
    https://archive.example/left.tar.gz|https://archive.example/right.tar.gz|https://archive.example/values.tar.gz|https://archive.example/future.tar.gz) ;;
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
    let program = "import left; import right; fn adjustment() -> Int { return 0; } fn main() { if left.value() + adjustment() != right.value() + adjustment() { panic(\"different value\"); } }";
    let app = fixture.path("app/src/main.rinzu");
    fs::write(&app, program).unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&fixture.path("app")).unwrap();
    assert_eq!(artifact.sdk_packages.len(), 3);
    let run = || {
        fixture
            .command(Path::new(env!("CARGO_BIN_EXE_rinzu")))
            .args(["run", "."])
            .env("RINZU_COMPILE_TIMINGS", "1")
            .output()
            .unwrap()
    };
    success(run());
    fs::write(&app, program.replace("return 0;", "return 1;")).unwrap();
    let edited = run();
    for name in ["values", "left", "right"] {
        assert!(timing_phase(&edited, "frontend.sdk-cache.hit", name));
        assert!(!timing_phase(&edited, "frontend.lower", name));
    }
    success(edited);
    fs::write(&app, "import values; fn main() {}").unwrap();
    assert!(rinzu_compiler::check_package(&fixture.path("app")).is_err());
    assert!(!run().status.success());
    success(fixture.cli(&["modules", "sync", "--offline"]));
    assert_eq!(
        fs::read_to_string(fixture.path("requests")).unwrap(),
        requests
    );

    let future = archive(
        &fixture.path("future.tar.gz"),
        "future",
        &format!(
            "{}\n[requires]\ntoolchain = \"99.0.0\"\n",
            SDK.replace("values", "future")
        ),
        SOURCE,
    );
    archive(
        &fixture.path("parent.tar.gz"),
        "parent",
        &format!(
            "{}\n[dependencies]\nfuture = {{ archive = \"https://archive.example/future.tar.gz\", integrity = \"sha256:{future}\" }}\n",
            SDK.replace("values", "parent")
        ),
        SOURCE,
    );
    let manifest = fixture.path("app/rinzu.toml");
    let lock = fixture.path("app/rinzu.modules.lock");
    let previous = (fs::read(&manifest).unwrap(), fs::read(&lock).unwrap());
    rejected(
        fixture.cli(&["modules", "add", "parent", "../parent.tar.gz"]),
        "Rinzu 99.0.0 or later is required (app -> parent -> future)",
    );
    assert_eq!(
        (fs::read(manifest).unwrap(), fs::read(lock).unwrap()),
        previous
    );
}

#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
))]
#[test]
fn path_sdk_frontends_reuse_app_edits_and_follow_current_dependency_sources() {
    let fixture = Fixture::new();
    let leaf = fixture.path("values/src/values.rinzu");
    let parent_manifest = fixture.path("parent/rinzu.toml");
    let parent = format!(
        "{}\n[dependencies]\nvalues = {{ path = \"../values\" }}\n",
        SDK.replace("values", "parent")
    );
    fs::create_dir_all(leaf.parent().unwrap()).unwrap();
    fs::create_dir_all(fixture.path("parent/src")).unwrap();
    fs::write(fixture.path("values/rinzu.toml"), SDK).unwrap();
    fs::write(&leaf, SOURCE).unwrap();
    fs::write(&parent_manifest, &parent).unwrap();
    fs::write(
        fixture.path("parent/src/parent.rinzu"),
        "import values; fn answer() -> Int { return values.value(); } export answer;",
    )
    .unwrap();
    let manifest = fixture.path("app/rinzu.toml");
    let dependencies = fs::read_to_string(&manifest).unwrap()
        + "\n[dependencies]\nparent = { path = \"../parent\" }\n";
    fs::write(manifest, dependencies).unwrap();
    let app = fixture.path("app/src/main.rinzu");
    let program = "import parent; import rinzu.stdio; fn adjustment() -> Int { return 0; } fn main() { try { rinzu.stdio.writeLine(parent.answer() + adjustment()); } catch errors { panic(\"write failed\"); } }";
    fs::write(&app, program).unwrap();

    let run = |expected: &[u8], phase: &str| {
        let output = fixture
            .command(Path::new(env!("CARGO_BIN_EXE_rinzu")))
            .args(["run", "."])
            .env("RINZU_COMPILE_TIMINGS", "1")
            .output()
            .unwrap();
        for name in ["values", "parent"] {
            assert!(
                timing_phase(&output, phase, name),
                "{name} missing {phase}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let regenerated = phase != "frontend.sdk-cache.hit";
            assert_eq!(timing_phase(&output, "frontend.sema", name), regenerated);
            assert_eq!(timing_phase(&output, "frontend.lower", name), regenerated);
        }
        assert_eq!(output.stdout, expected);
        success(output);
    };
    run(b"42\n", "frontend.sdk-cache.store");
    let edited_program = program.replace("return 0;", "return 1;");
    fs::write(&app, &edited_program).unwrap();
    run(b"43\n", "frontend.sdk-cache.hit");

    // The same-length leaf edit changes both the leaf and the parent's checked input.
    fs::write(&leaf, SOURCE.replace("42", "43")).unwrap();
    run(b"44\n", "frontend.sdk-cache.miss");

    // Discovery includes source files without an import edge from the entry.
    let added = fixture.path("values/src/extra.rinzu");
    let moved = fixture.path("values/src/moved.rinzu");
    fs::write(&added, "fn extra() -> Int { return 2; }").unwrap();
    run(b"44\n", "frontend.sdk-cache.miss");
    fs::rename(&added, &moved).unwrap();
    run(b"44\n", "frontend.sdk-cache.miss");
    fs::write(&moved, "fn extra() -> Int { return false; }").unwrap();
    rejected(fixture.cli(&["run", "."]), "moved.rinzu");
    fs::remove_file(&moved).unwrap();

    // Relocating the SDK invalidates its saved paths and file IDs even with the same bytes.
    fs::rename(fixture.path("values"), fixture.path("relocated-values")).unwrap();
    fs::write(
        parent_manifest,
        parent.replace("../values", "../relocated-values"),
    )
    .unwrap();
    run(b"44\n", "frontend.sdk-cache.miss");
    fs::write(&app, program).unwrap();
    run(b"43\n", "frontend.sdk-cache.hit");
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn locked_framework_sdk_restores_before_abi_and_linking_and_rechecks_headers() {
    let fixture = Fixture::new();
    let archive_path = fixture.path("desktop.tar.gz");
    archive_with_files(
        &archive_path,
        "desktop",
        r#"[package]
name = "desktop"
language = "0.5"
kind = "sdk"

[native]
sources = ["native/api.m"]
headers = ["native/api.h"]
frameworks = ["Foundation"]
"#,
        "native { fn value() -> Int symbol \"early_sdk_value\"; } export value;",
        &[
            (
                "native/api.h",
                "#include <stdint.h>\n#include <rinzu_early_sdk_fixture.h>\nEARLY_TYPE early_sdk_value(void);",
            ),
            (
                "native/api.m",
                "EARLY_TYPE early_sdk_value(void) { return EARLY_VALUE; }",
            ),
        ],
    );
    success(fixture.cli(&["modules", "add", "desktop", "../desktop.tar.gz"]));
    for directory in ["preferred", "headers"] {
        fs::create_dir(fixture.path(directory)).unwrap();
    }
    let header = "rinzu_early_sdk_fixture.h";
    let definitions =
        |kind: &str, value| format!("#define EARLY_TYPE {kind}\n#define EARLY_VALUE {value}\n");
    fs::write(
        fixture.path(&format!("headers/{header}")),
        definitions("int64_t", 42),
    )
    .unwrap();
    let includes =
        std::env::join_paths([fixture.path("preferred"), fixture.path("headers")]).unwrap();
    let app = fixture.path("app/src/main.rinzu");
    let program = r#"import desktop;
import rinzu.stdio;
import rinzu.timer;
fn adjustment() -> Int { return 0; }
async fn main() {
    try {
        await rinzu.timer.sleep(1);
        rinzu.stdio.writeLine(desktop.value() + adjustment());
    } catch errors {
        panic("native call failed");
    }
}"#;
    fs::write(&app, program).unwrap();
    let run = || {
        fixture
            .command(Path::new(env!("CARGO_BIN_EXE_rinzu")))
            .args(["run", "."])
            .env("RINZU_COMPILE_TIMINGS", "1")
            .env("CPATH", &includes)
            .output()
            .unwrap()
    };
    let cold = run();
    assert_eq!(cold.stdout, b"42\n");
    assert!(timing_phase(
        &cold,
        "native.package.link.provider",
        "desktop"
    ));
    assert!(timing_phase(&cold, "native.package.early.store", "desktop"));
    assert!(timing_phase(&cold, "frontend.sdk-cache.store", "desktop"));
    success(cold);
    let warm = run();
    assert_eq!(warm.stdout, b"42\n");
    assert!(
        timing_phase(&warm, "native.package.early.hit", "desktop"),
        "{}",
        String::from_utf8_lossy(&warm.stderr)
    );
    assert!(!timing_phase(
        &warm,
        "native.package.link.provider",
        "desktop"
    ));
    assert!(!timing_phase(&warm, "native.contract.validate", "desktop"));
    success(warm);

    fs::write(&app, program.replace("return 0;", "return 1;")).unwrap();
    let edited = run();
    assert_eq!(edited.stdout, b"43\n");
    assert!(timing_phase(&edited, "native.package.early.hit", "desktop"));
    assert!(timing_phase(&edited, "frontend.sdk-cache.hit", "desktop"));
    assert!(!timing_phase(&edited, "frontend.sema", "desktop"));
    assert!(!timing_phase(&edited, "frontend.lower", "desktop"));
    success(edited);

    let frontend = fs::read_dir(fixture.path("cache/jit-sdk-frontend-v1"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("interface.msgpack");
    fs::write(frontend, b"corrupt checked SDK").unwrap();
    fs::write(&app, program.replace("return 0;", "return 2;")).unwrap();
    let rebuilt = run();
    assert_eq!(rebuilt.stdout, b"44\n");
    assert!(timing_phase(&rebuilt, "frontend.sdk-cache.miss", "desktop"));
    assert!(timing_phase(&rebuilt, "frontend.lower", "desktop"));
    assert!(timing_phase(
        &rebuilt,
        "frontend.sdk-cache.store",
        "desktop"
    ));
    success(rebuilt);
    fs::write(&app, program.replace("return 0;", "return 1;")).unwrap();

    let cache = fs::read_dir(fixture.path("cache/source-v4"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("early-sdk-inputs.json").is_file())
        .unwrap();
    let provider = fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("provider-")
        })
        .unwrap();
    fs::write(provider, b"corrupt provider").unwrap();
    let repaired = run();
    assert_eq!(repaired.stdout, b"43\n");
    assert!(timing_phase(
        &repaired,
        "native.package.link.provider",
        "desktop"
    ));
    assert!(!timing_phase(
        &repaired,
        "native.package.early.hit",
        "desktop"
    ));
    success(repaired);

    let preferred = fixture.path(&format!("preferred/{header}"));
    fs::write(&preferred, definitions("int64_t", 44)).unwrap();
    let shadowed = run();
    assert_eq!(shadowed.stdout, b"45\n");
    assert!(timing_phase(
        &shadowed,
        "native.package.early.miss",
        "ABI preprocessing changed"
    ));
    assert!(timing_phase(
        &shadowed,
        "native.package.link.provider",
        "desktop"
    ));
    success(shadowed);

    fs::write(preferred, definitions("int32_t", 44)).unwrap();
    fs::write(&app, program).unwrap();
    let invalid_abi = run();
    assert!(timing_phase(
        &invalid_abi,
        "frontend.sdk-cache.hit",
        "desktop"
    ));
    rejected(invalid_abi, "native scalar ABI mismatch");
}
