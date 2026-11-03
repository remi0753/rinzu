use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rinzu_compiler::{
    CompilationMode, CompilationOptions, DiagnosticCode, FileId, NativeCliInputs, SourceCache,
    SourceFile, SourceSnapshot, check_input, check_input_with_sources, format_source,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rinzu-m5-file-frontend-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn write(&self, path: &str, text: &str) -> PathBuf {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let formatted = format_source(&SourceFile::new(FileId(1), &path, text)).unwrap();
        std::fs::write(&path, formatted).unwrap();
        path
    }
    fn package(&self) {
        self.write(
            "src/chosen.rinzu",
            "fn fromPackage() {} fn main() { fromPackage(); }",
        );
        std::fs::write(self.0.join("rinzu.toml"),
            "[package]\nname = \"selected_package\"\nlanguage = \"0.3\"\nentry = \"src/chosen.rinzu\"\n").unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn options() -> CompilationOptions {
    CompilationOptions {
        language: None,
        target: "frontend-test-target".into(),
    }
}
fn check(
    path: &Path,
) -> Result<rinzu_compiler::CompilationContext, Vec<rinzu_compiler::Diagnostic>> {
    check_input(path, &options(), &NativeCliInputs::default())
}

#[test]
fn missing_entry_is_accepted_only_from_an_explicit_source_overlay() {
    let scratch = Scratch::new();
    let entry = scratch.0.join("unsaved.rinzu");
    let dependency = scratch.write("model.rinzu", "fn value()->Int{return 42;}export value;");
    let text = format_source(&SourceFile::new(
        FileId(1),
        &entry,
        "import \"./model.rinzu\" as model;fn main(){let n:Int=model.value();}",
    ))
    .unwrap();
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(entry.clone(), text.into_bytes())]),
    );
    let context = check_input_with_sources(
        &entry,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .unwrap();
    assert_eq!(context.mode, CompilationMode::File);
    assert_eq!(context.entry, entry);
    assert_eq!(context.artifact.module_graph.len(), 2);
    assert!(
        context
            .artifact
            .module_graph
            .iter()
            .any(|node| node.path == dependency)
    );
    assert!(
        snapshot
            .input_receipts()
            .iter()
            .any(|input| input.0 == entry && input.1 == "overlay")
    );
    assert!(!entry.exists());
    assert!(
        check(&entry).unwrap_err()[0]
            .message
            .contains("cannot inspect input")
    );
}

#[test]
fn explicit_unicode_file_ignores_nearby_manifest_lock_and_src_entry() {
    let scratch = Scratch::new();
    scratch.package();
    std::fs::write(
        scratch.0.join("rinzu.lock"),
        "deliberately unrelated invalid lock",
    )
    .unwrap();
    let path = scratch.write("任意 name.rinzu", "import rinzu.stdio; fn onlyFile() {} fn main() { onlyFile(); try{rinzu.stdio.writeLine(\"file\");}catch errors{} }");
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
    let context = check_input_with_sources(
        &path,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .unwrap();
    assert_eq!(context.mode, CompilationMode::File);
    assert_eq!(context.entry, path.canonicalize().unwrap());
    assert_eq!(context.metadata.name, "任意 name");
    assert_eq!(context.metadata.language, "0.5");
    assert_eq!(context.target, "frontend-test-target");
    assert_eq!(
        context.default_output(),
        std::env::current_dir().unwrap().join("build/任意 name")
    );
    let names: Vec<_> = context
        .artifact
        .checked
        .functions
        .iter()
        .map(|f| f.declaration.name.as_str())
        .collect();
    assert!(names.contains(&"onlyFile") && !names.contains(&"fromPackage"));
    assert_eq!(snapshot.input_receipts().len(), 1);
    assert_eq!(snapshot.input_receipts()[0].1, "disk-source");
    assert_eq!(
        context
            .artifact
            .ir
            .source_files
            .values()
            .collect::<Vec<_>>(),
        vec![&path.canonicalize().unwrap().display().to_string()]
    );
}

#[test]
fn directory_preserves_package_selection_and_language() {
    let scratch = Scratch::new();
    scratch.package();
    scratch.write("outside.rinzu", "fn main() {}");
    let context = check(&scratch.0).unwrap();
    assert_eq!(context.mode, CompilationMode::Package);
    assert_eq!(context.metadata.name, "selected_package");
    assert_eq!(context.metadata.language, "0.3");
    assert_eq!(
        context.entry,
        scratch.0.join("src/chosen.rinzu").canonicalize().unwrap()
    );
    assert_eq!(
        context.default_output(),
        scratch.0.join("build/selected_package")
    );
    let overridden = CompilationOptions {
        language: Some("0.5".into()),
        ..options()
    };
    assert!(
        check_input(&scratch.0, &overridden, &NativeCliInputs::default()).unwrap_err()[0]
            .message
            .contains("cannot override")
    );
    // The legacy package API remains available for manifest callers.
    assert!(rinzu_compiler::check_package(&scratch.0.join("rinzu.toml")).is_ok());
    assert!(
        check(&scratch.0.join("rinzu.toml")).unwrap_err()[0]
            .message
            .contains("configuration")
    );
}

#[test]
fn async_and_unconventional_filename_use_existing_typed_entry_and_core_resolution() {
    let scratch = Scratch::new();
    let path = scratch.write("自由な program", "import rinzu.core; async fn ready()->Int { return 1; } async fn main() { let pending=ready(); let isReady=rinzu.core.futureReady(pending); let value=await pending; let accepted=setProcessExitStatus(7); }");
    let context = check(&path).unwrap();
    assert!(
        context
            .artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "main")
            .unwrap()
            .facts
            .is_async
    );
    assert!(rinzu_compiler::verify_module(&context.artifact.ir).is_ok());
    let explicit = CompilationOptions {
        language: Some("0.5".into()),
        ..options()
    };
    assert!(check_input(&path, &explicit, &NativeCliInputs::default()).is_ok());
    let legacy = CompilationOptions {
        language: Some("0.1".into()),
        ..options()
    };
    assert!(
        check_input(&path, &legacy, &NativeCliInputs::default()).unwrap_err()[0]
            .message
            .contains("0.5 only")
    );
    assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 1);
}

#[test]
fn main_and_type_errors_belong_to_the_explicit_file() {
    let scratch = Scratch::new();
    scratch.package();
    for (text, expected) in [
        ("fn helper() {}", DiagnosticCode::InvalidEntryPoint),
        ("fn main(value:Int) {}", DiagnosticCode::InvalidEntryPoint),
        (
            "fn main()->Int { return 1; }",
            DiagnosticCode::InvalidEntryPoint,
        ),
        (
            "fn main() { let value:Int=\"bad\"; }",
            DiagnosticCode::TypeMismatch,
        ),
    ] {
        let path = scratch.write("outside.rinzu", text);
        let errors = check(&path).unwrap_err();
        assert!(errors.iter().any(|e| e.code == expected), "{errors:?}");
        assert!(errors.iter().all(|e| e.primary.file_id != FileId::MANIFEST));
    }
    let path = scratch.0.join("outside.rinzu");
    // Intentional parser-error vector; not formatter input.
    std::fs::write(&path, "fn main( {").unwrap();
    assert!(check(&path).is_err());
    assert!(check(&scratch.0).is_ok());
    assert!(
        check(&scratch.0.join("missing.rinzu")).unwrap_err()[0]
            .message
            .contains("cannot inspect")
    );
}

#[test]
fn snapshot_overlay_and_parse_cache_keep_physical_identity_without_configuration() {
    let scratch = Scratch::new();
    let path = scratch
        .write("main.rinzu", "fn diskOnly() {} fn main() { diskOnly(); }")
        .canonicalize()
        .unwrap();
    let overlay = format_source(&SourceFile::new(
        FileId(1),
        &path,
        "fn overlayOnly() {} fn main() { overlayOnly(); }",
    ))
    .unwrap();
    let mut cache = SourceCache::default();
    let mut first = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(path.clone(), overlay.clone().into_bytes())]),
    );
    let context =
        check_input_with_sources(&path, &options(), &NativeCliInputs::default(), &mut first)
            .unwrap();
    let id = context.artifact.syntax.file_id;
    assert!(
        context
            .artifact
            .checked
            .functions
            .iter()
            .any(|f| f.declaration.name == "overlayOnly")
    );
    assert_eq!(first.input_receipts()[0].1, "overlay");
    drop(first);
    let mut second = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(path.clone(), overlay.into_bytes())]),
    );
    let again =
        check_input_with_sources(&path, &options(), &NativeCliInputs::default(), &mut second)
            .unwrap();
    assert_eq!(again.artifact.syntax.file_id, id);
    assert_eq!(second.parse_hits, 1);
}

#[cfg(unix)]
#[test]
fn symlink_entry_is_canonical_and_broken_link_does_not_select_package() {
    let scratch = Scratch::new();
    scratch.package();
    let path = scratch.write("real program.rinzu", "fn main() {}");
    let alias = scratch.0.join("alias.rinzu");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert_eq!(check(&alias).unwrap().entry, path.canonicalize().unwrap());
    let broken = scratch.0.join("broken.rinzu");
    std::os::unix::fs::symlink("missing.rinzu", &broken).unwrap();
    assert!(check(&broken).is_err());
}

#[test]
fn unresolved_import_and_native_inputs_do_not_borrow_package_dependencies() {
    let scratch = Scratch::new();
    scratch.package();
    scratch.write(
        "src/local/module.rinzu",
        "fn value()->Int{return 42;}export value;",
    );
    let path = scratch.write("main.rinzu", "import local.module; fn main() {}");
    assert!(
        check(&path).unwrap_err()[0]
            .message
            .contains(&scratch.0.join("local/module.rinzu").display().to_string())
    );
    let path = scratch.write("main.rinzu", "fn main() {}");
    let inputs = NativeCliInputs {
        libraries: vec!["System".into()],
        ..NativeCliInputs::default()
    };
    let context = check_input(&path, &options(), &inputs).unwrap();
    assert_eq!(context.artifact.native_inputs.libraries, ["System"]);
    assert!(context.artifact.sdk_packages.is_empty());
}
