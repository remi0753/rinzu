use rinzu_compiler::{
    CompilationOptions, FileId, NativeCliInputs, SourceFile, check_input, format_source,
    standard_library,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-stdlib-frontend-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn source(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let formatted = format_source(&SourceFile::new(FileId(1), &path, text)).unwrap();
        std::fs::write(&path, formatted).unwrap();
        path
    }
    fn manifest(&self, dependencies: &str) {
        std::fs::write(self.0.join("rinzu.toml"), format!(
            "[package]\nname=\"stdlib_app\"\nlanguage=\"0.5\"\nentry=\"src/main.rinzu\"\n{dependencies}"
        )).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(path: &Path) -> rinzu_compiler::CompilationContext {
    check_input(
        path,
        &CompilationOptions {
            language: None,
            target: standard_library::TARGET.into(),
        },
        &NativeCliInputs::default(),
    )
    .unwrap_or_else(|errors| panic!("{errors:#?}"))
}

fn packages(context: &rinzu_compiler::CompilationContext) -> Vec<String> {
    context
        .artifact
        .sdk_packages
        .iter()
        .map(|p| p.manifest.name.clone())
        .collect()
}

#[test]
fn every_selected_source_package_is_reachable_without_manifest_dependencies() {
    let scratch = Scratch::new();
    for name in standard_library::SOURCE_PACKAGES {
        let entry = scratch.source("entry.rinzu", &format!("import {name}; fn main() {{}}"));
        let result = check(&entry);
        let expected: Vec<_> = standard_library::dependencies(name)
            .iter()
            .copied()
            .chain([name])
            .collect();
        assert_eq!(packages(&result), expected);
        assert!(result.artifact.source_interface.is_none());
        assert!(
            result
                .artifact
                .checked_inputs
                .keys()
                .any(|p| p.ends_with("registry.json"))
        );
    }
}

#[test]
fn file_and_package_share_standard_nominal_types_and_diamond_identity() {
    let scratch = Scratch::new();
    scratch.manifest("");
    let entry = scratch.source("src/main.rinzu", "import bridge; import rinzu.io; import rinzu.stream; fn convert(value: bridge.File) -> rinzu.io.File { return value; } fn main() {}");
    scratch.source("src/bridge.rinzu", "import rinzu.io; export rinzu.io.File;");
    let file = check(&entry);
    let package = check(&scratch.0);
    assert_eq!(packages(&file), ["rinzu.io", "rinzu.stream"]);
    assert_eq!(packages(&file), packages(&package));
    let io_file = |c: &rinzu_compiler::CompilationContext| {
        c.artifact.sdk_packages[0].source.as_ref().unwrap().modules["rinzu.io"]["File"].clone()
    };
    assert_eq!(io_file(&file), io_file(&package));
    assert!(file.artifact.source_interface.is_none());
}

#[test]
fn path_import_discovers_standard_dependencies_without_loading_unrelated_native_packages() {
    let scratch = Scratch::new();
    let entry = scratch.source(
        "entry.rinzu",
        "import \"nested/helper.rinzu\" as helper; fn main() {try{helper.test();}catch errors{}}",
    );
    scratch.source(
        "nested/helper.rinzu",
        "import rinzu.testing; fn test() { rinzu.testing.assertTrue(true, \"ok\"); } export test;",
    );
    assert_eq!(packages(&check(&entry)), ["rinzu.testing"]);
    let builtin = scratch.source(
        "builtin.rinzu",
        "import rinzu.stdio; fn main() { try{rinzu.stdio.writeLine(1);}catch errors{} }",
    );
    assert!(packages(&check(&builtin)).is_empty());
}

#[test]
fn additional_sdk_can_reexport_standard_types_without_duplicating_the_standard_owner() {
    let scratch = Scratch::new();
    scratch.manifest("[dependencies]\n\"rinzu.custom\"={path=\"sdk\"}\n");
    scratch.source("src/main.rinzu", "import rinzu.custom; import rinzu.io; fn convert(v: rinzu.custom.File)->rinzu.io.File { return v; } fn main() {}");
    scratch.source(
        "sdk/src/main.rinzu",
        "import rinzu.io; export rinzu.io.File;",
    );
    std::fs::write(scratch.0.join("sdk/rinzu.toml"), "[package]\nname=\"rinzu.custom\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/main.rinzu\"\n").unwrap();
    assert_eq!(packages(&check(&scratch.0)), ["rinzu.io", "rinzu.custom"]);
}

#[test]
fn standard_names_reject_overrides_but_other_rinzu_names_remain_available() {
    let scratch = Scratch::new();
    scratch.source("src/main.rinzu", "fn main() {}");
    for name in standard_library::names() {
        scratch.manifest(&format!(
            "[dependencies]\n\"{name}\"={{path=\"missing\"}}\n"
        ));
        let errors = check_input(
            &scratch.0,
            &CompilationOptions {
                language: None,
                target: standard_library::TARGET.into(),
            },
            &NativeCliInputs::default(),
        )
        .unwrap_err();
        assert!(
            errors.iter().any(|e| e
                .message
                .contains("toolchain module and cannot be replaced")),
            "{errors:?}"
        );
    }
    let entry = scratch.source("unknown.rinzu", "import rinzu.unregistered; fn main() {}");
    let errors = check_input(
        &entry,
        &CompilationOptions {
            language: None,
            target: standard_library::TARGET.into(),
        },
        &NativeCliInputs::default(),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("rinzu.unregistered")
                && e.primary.file_id != FileId::MANIFEST)
    );
}

#[test]
fn socket_handles_are_nominal_and_cannot_cross_worker_domains() {
    let scratch = Scratch::new();
    for (case, source) in [
        (
            "direction",
            "import rinzu.socket; async fn wrong(value: rinzu.socket.Listener) { let result = await rinzu.socket.read(value, 1); } fn main() {}",
        ),
        (
            "domain",
            "import rinzu.socket; fn worker(value: rinzu.socket.TcpStream) {} async fn wrong(value: rinzu.socket.TcpStream) { await spawn worker(value); } fn main() {}",
        ),
        (
            "private",
            "import rinzu.socket; fn main() { let op = rinzu.socket.rawOperation(1); }",
        ),
    ] {
        let entry = scratch.source(&format!("{case}.rinzu"), source);
        let errors = check_input(
            &entry,
            &CompilationOptions {
                language: None,
                target: standard_library::TARGET.into(),
            },
            &NativeCliInputs::default(),
        )
        .unwrap_err();
        assert!(!errors.is_empty(), "accepted {case}");
        assert!(
            errors.iter().any(|e| e.primary.file_id != FileId::MANIFEST),
            "{errors:?}"
        );
    }
    let entry = scratch.source("valid.rinzu", "import rinzu.socket; async fn read(value: rinzu.socket.TcpStream) { let result = await rinzu.socket.read(value, 1); } fn main() {}");
    assert_eq!(packages(&check(&entry)), ["rinzu.socket"]);
}

#[test]
fn standard_source_requires_current_language_and_supported_target() {
    let scratch = Scratch::new();
    let entry = scratch.source("src/main.rinzu", "import rinzu.testing; fn main() {}");
    let errors = check_input(
        &entry,
        &CompilationOptions {
            language: None,
            target: "unsupported".into(),
        },
        &NativeCliInputs::default(),
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("requested unsupported"))
    );
    scratch.manifest("");
    let manifest = scratch.0.join("rinzu.toml");
    std::fs::write(
        &manifest,
        std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("0.5", "0.4"),
    )
    .unwrap();
    let errors = rinzu_compiler::check_package(&scratch.0).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("requires language 0.5"))
    );
}

#[test]
fn additional_sdk_public_module_cannot_shadow_a_registered_standard_module() {
    let scratch = Scratch::new();
    scratch.manifest("[dependencies]\n\"rinzu\"={path=\"sdk\"}\n");
    scratch.source("src/main.rinzu", "import rinzu.io; fn main() {}");
    scratch.source(
        "sdk/src/main.rinzu",
        "fn value()->Int {return 1;} export value;",
    );
    scratch.source(
        "sdk/src/io.rinzu",
        "fn value()->Int {return 2;} export value;",
    );
    std::fs::write(scratch.0.join("sdk/rinzu.toml"), "[package]\nname=\"rinzu\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/main.rinzu\"\n[exports]\nmodules=[\"main\",\"io\"]\n").unwrap();
    let errors = rinzu_compiler::check_package(&scratch.0).unwrap_err();
    assert!(
        errors.iter().any(|e| e
            .message
            .contains("conflicts with a registered standard module")),
        "{errors:?}"
    );
}

#[test]
fn sdk_task_control_is_valid_for_sync_and_async_entries() {
    let scratch = Scratch::new();
    let entry = scratch.source(
        "entry.rinzu",
        "import rinzu.process; fn main() { let id=rinzu.process.identifier(); }",
    );
    check(&entry);
    scratch.source(
        "entry.rinzu",
        "import rinzu.process; fn main() { let accepted=rinzu.process.setExitStatus(7); }",
    );
    check(&entry);
    scratch.source(
        "entry.rinzu",
        "import rinzu.process; async fn main() { let accepted=rinzu.process.setExitStatus(7); }",
    );
    check(&entry);
}

#[test]
fn resource_inventory_detects_byte_changes_and_missing_files() {
    let scratch = Scratch::new();
    standard_library::write_resources(&scratch.0).unwrap();
    standard_library::verify(&scratch.0).unwrap();
    let path = scratch.0.join("packages/testing/src/testing.rinzu");
    std::fs::write(&path, "fn changed() {}").unwrap();
    assert!(
        standard_library::verify(&scratch.0)
            .unwrap_err()
            .contains("changed or invalid")
    );
    std::fs::remove_file(path).unwrap();
    assert!(standard_library::verify(&scratch.0).is_err());
}

#[test]
fn registry_reuse_preserves_embedded_inventory_and_caller_isolation() {
    let expected = standard_library::registry_bytes();
    let mut caller = standard_library::registry_bytes();
    caller.clear();
    assert_eq!(standard_library::registry_bytes(), expected);
    let registry: serde_json::Value = serde_json::from_slice(&expected).unwrap();
    for (name, bytes) in standard_library::resources() {
        assert_eq!(registry["files"][name], standard_library::sha256(bytes));
    }
}

#[test]
fn file_native_owner_keeps_only_its_checked_graph_when_importing_standard_source() {
    let scratch = Scratch::new();
    let entry = scratch.source("entry.rinzu", "import rinzu.shutdown; native {fn localClock()->Int symbol \"custom_clock\";} fn main() {let value=localClock();let standard=rinzu.shutdown.clockMillis();}");
    let result = check(&entry);
    let own = result.artifact.source_interface.unwrap();
    assert_eq!(own.module_graph.len(), 1);
    assert!(own.module_graph.iter().all(|node| {
        own.source_inputs
            .contains_key(&node.path.display().to_string())
    }));
    assert_eq!(result.artifact.module_graph.len(), 2);
}

#[test]
fn registry_and_distribution_fingerprint_share_runtime_codegen_revision() {
    let registry: serde_json::Value =
        serde_json::from_slice(&standard_library::registry_bytes()).unwrap();
    assert_eq!(
        registry["runtimeCodegenRevision"],
        standard_library::RUNTIME_CODEGEN_REVISION
    );
    assert_eq!(
        standard_library::fingerprint()["runtimeCodegenRevision"],
        registry["runtimeCodegenRevision"]
    );
}
