use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rinzu_compiler::{
    CompilationContext, CompilationOptions, Diagnostic, DiagnosticCode, FileId, NativeCliInputs,
    SourceCache, SourceFile, SourceSnapshot, check_input, check_input_with_sources, format_source,
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rinzu-path-graph-{}-{}-{}",
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
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text = format_source(&SourceFile::new(FileId(1), &path, text)).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
    fn package(&self, name: &str, kind: &str, extra: &str) -> PathBuf {
        let root = self.0.join(name);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("rinzu.toml"),
            format!("[package]\nname=\"{name}\"\nlanguage=\"0.5\"\nkind=\"{kind}\"\nentry=\"src/main.rinzu\"\n{extra}"),
        )
        .unwrap();
        root
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
        target: "graph-test-target".into(),
    }
}
fn check(path: &Path) -> Result<CompilationContext, Vec<Diagnostic>> {
    check_input(path, &options(), &NativeCliInputs::default())
}

#[test]
fn hierarchy_uses_each_importer_and_all_sources_enter_the_receipt() {
    let scratch = Scratch::new();
    let value = scratch.write(
        "outside/日本語 file.rinzu",
        "fn answer()->Int{return 42;}export answer;",
    );
    scratch.write("app/lib/child.rinzu", "import \"../../outside/日本語 file.rinzu\" as value;async fn answer()->Int{return value.answer();}export answer;");
    let entry = scratch.write("app/任意 entry.rinzu", "import \"./lib/child.rinzu\" as child;import rinzu.stdio;async fn main(){try{let n=await child.answer();rinzu.stdio.writeLine(f\"answer = {n}\");}catch errors{}}");
    std::fs::write(
        scratch.0.join("app/rinzu.toml"),
        "unrelated invalid manifest",
    )
    .unwrap();
    let context = check(&entry).unwrap();
    assert_eq!(context.entry, entry);
    assert_eq!(context.artifact.module_graph.len(), 3);
    assert_eq!(context.inputs.len(), 3);
    assert!(
        context
            .inputs
            .iter()
            .all(|input| input.kind == "disk-source")
    );
    assert!(
        context
            .artifact
            .ir
            .source_files
            .values()
            .any(|path| path == &value.display().to_string())
    );
    assert_eq!(
        context
            .artifact
            .module_graph
            .iter()
            .filter(|node| node.is_entry)
            .count(),
        1
    );
    assert!(
        context
            .artifact
            .module_graph
            .iter()
            .flat_map(|node| &node.imports)
            .any(|edge| edge.kind == "builtin")
    );
}

#[test]
fn aliases_dotted_paths_and_symlinks_share_one_nominal_type_and_body() {
    let scratch = Scratch::new();
    let model = scratch.write("model.rinzu", "class Pair{x:Int}fn take(p:Pair)->Int{return p.x;}fn main()->Int{return 99;}export Pair;export take;");
    std::os::unix::fs::symlink(&model, scratch.0.join("alternate.rinzu")).unwrap();
    let entry = scratch.write("main.rinzu", "import model;import \"./alternate.rinzu\" as other;import \"./model.rinzu\" as direct;fn main(){let p:model.Pair=direct.Pair(x:42);let n=other.take(p);}");
    let context = check(&entry).unwrap();
    assert_eq!(context.artifact.module_graph.len(), 2);
    assert_eq!(context.inputs.len(), 2);
    let node = context
        .artifact
        .module_graph
        .iter()
        .find(|node| node.is_entry)
        .unwrap();
    let paths: Vec<_> = node
        .imports
        .iter()
        .map(|edge| edge.target_path.as_ref().unwrap())
        .collect();
    assert!(paths.iter().all(|path| *path == &model));
    assert!(
        node.imports
            .iter()
            .all(|edge| edge.public_symbols["Pair"] == "model::Pair")
    );
    assert_eq!(
        context
            .artifact
            .checked
            .functions
            .iter()
            .filter(|f| f.declaration.name == "model::take")
            .count(),
        1
    );
    assert_eq!(
        context
            .artifact
            .checked
            .functions
            .iter()
            .filter(|f| f.declaration.name == "main")
            .count(),
        1
    );
    assert!(
        context
            .artifact
            .checked
            .functions
            .iter()
            .any(|f| f.declaration.name == "model::main")
    );
}

#[test]
fn package_and_sdk_public_module_names_preserve_canonical_identity() {
    let scratch = Scratch::new();
    scratch.package(
        "model",
        "sdk",
        "[exports]\nmodules=[\"main\",\"alternate\"]\n",
    );
    let implementation = scratch.write(
        "model/src/implementation.rinzu",
        "class Pair{x:Int}fn take(p:Pair)->Int{return p.x;}export Pair;export take;",
    );
    std::os::unix::fs::symlink(&implementation, scratch.0.join("model/src/alternate.rinzu"))
        .unwrap();
    scratch.write(
        "model/src/main.rinzu",
        "import \"./implementation.rinzu\" as impl;export impl.Pair;export impl.take;",
    );
    let app = scratch.package(
        "app",
        "application",
        "[dependencies]\nmodel={path=\"../model\"}\n",
    );
    scratch.write("app/src/main.rinzu", "import model;import model.alternate;fn main(){let p:model.Pair=model.Pair(x:42);let n=model.alternate.take(p);}");
    let context = check(&app).unwrap();
    let sdk = context.artifact.sdk_packages[0].source.as_ref().unwrap();
    assert_eq!(
        sdk.modules["model"]["Pair"],
        sdk.modules["model.alternate"]["Pair"]
    );
    assert_eq!(sdk.module_graph.len(), 2);
    assert_eq!(sdk.modules["model"]["Pair"], "model::implementation::Pair");
    assert!(
        context
            .artifact
            .module_graph
            .iter()
            .any(|node| node.path == implementation)
    );
}

#[test]
fn alias_conflicts_and_private_exports_are_rejected() {
    let scratch = Scratch::new();
    scratch.write(
        "one.rinzu",
        "fn value()->Int{return 1;}fn hidden()->Int{return 2;}export value;",
    );
    scratch.write("two.rinzu", "fn value()->Int{return 2;}export value;");
    for text in [
        "import \"./one.rinzu\" as m;import \"./two.rinzu\" as m;fn main(){}",
        "import \"./one.rinzu\" as one;import one;fn main(){}",
        "import \"./one.rinzu\" as m;fn m(){}fn main(){}",
    ] {
        let entry = scratch.write("main.rinzu", text);
        let errors = check(&entry).unwrap_err();
        assert_eq!(errors[0].code, DiagnosticCode::InvalidModulePath);
        assert!(!errors[0].secondary.is_empty());
    }
    let entry = scratch.write(
        "main.rinzu",
        "import \"./one.rinzu\" as m;import \"././one.rinzu\" as m;fn main(){let n=m.value();}",
    );
    assert_eq!(check(&entry).unwrap().artifact.module_graph.len(), 2);
    let entry = scratch.write(
        "main.rinzu",
        "import \"./one.rinzu\" as m;fn main(){let n=m.hidden();}",
    );
    assert!(
        check(&entry)
            .unwrap_err()
            .iter()
            .any(|error| error.code == DiagnosticCode::UnknownName)
    );
}

#[test]
fn cycle_and_nested_type_error_keep_physical_positions_and_import_chain() {
    let scratch = Scratch::new();
    scratch.write(
        "lib/a.rinzu",
        "import \"../main.rinzu\" as entry;fn value()->Int{return 1;}export value;",
    );
    let entry = scratch.write("main.rinzu", "import \"./lib/a.rinzu\" as a;fn main(){}");
    let error = check(&entry).unwrap_err().remove(0);
    assert_eq!(error.code, DiagnosticCode::ImportCycle);
    assert!(error.message.contains(&entry.display().to_string()));
    assert!(!error.secondary.is_empty());
    let broken = scratch.write(
        "lib/b.rinzu",
        "fn value()->Int{return \"wrong\";}export value;",
    );
    scratch.write(
        "lib/a.rinzu",
        "import \"./b.rinzu\" as b;fn value()->Int{return b.value();}export value;",
    );
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
    let errors = check_input_with_sources(
        &entry,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .unwrap_err();
    let error = errors
        .iter()
        .find(|error| error.code == DiagnosticCode::TypeMismatch)
        .unwrap();
    assert_eq!(
        snapshot.path_for_id(error.primary.file_id),
        Some(broken.as_path())
    );
    assert_eq!(error.secondary.len(), 2);
    assert!(error.message.contains("import chain:"));
    assert!(error.message.contains(&broken.display().to_string()));
}

#[test]
fn overlays_virtual_dependencies_and_cached_parses_use_the_same_graph() {
    let scratch = Scratch::new();
    let model = scratch.write("model.rinzu", "fn value()->Int{return 1;}export value;");
    let entry = scratch.write(
        "main.rinzu",
        "import \"./model.rinzu\" as model;fn main(){let n=model.value();}",
    );
    let mut cache = SourceCache::default();
    let virtual_path = scratch.0.join("fresh.rinzu");
    let virtual_source = format_source(&SourceFile::new(
        FileId(1),
        &virtual_path,
        "fn value()->Int{return 42;}export value;",
    ))
    .unwrap();
    let edited = format_source(&SourceFile::new(
        FileId(1),
        &entry,
        "import \"./fresh.rinzu\" as model;fn main(){let n=model.value();}",
    ))
    .unwrap();
    let overlays = BTreeMap::from([
        (entry.clone(), edited.into_bytes()),
        (virtual_path.clone(), virtual_source.into_bytes()),
    ]);
    let mut snapshot = SourceSnapshot::new(&mut cache, overlays.clone());
    let first = check_input_with_sources(
        &entry,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .unwrap();
    assert_eq!(snapshot.parse_misses, 2);
    assert!(
        first
            .inputs
            .iter()
            .any(|input| input.path == virtual_path && input.kind == "overlay")
    );
    assert!(!first.inputs.iter().any(|input| input.path == model));
    drop(snapshot);
    let mut snapshot = SourceSnapshot::new(&mut cache, overlays);
    check_input_with_sources(
        &entry,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .unwrap();
    assert_eq!(snapshot.parse_hits, 2);
    assert!(!virtual_path.exists());
}

#[test]
fn package_boundary_and_imported_native_contract_remain_explicit() {
    let scratch = Scratch::new();
    let outside = scratch.write("outside.rinzu", "fn value()->Int{return 42;}export value;");
    let app = scratch.package("app", "application", "");
    scratch.write(
        "app/src/main.rinzu",
        "import \"../../outside.rinzu\" as value;fn main(){}",
    );
    assert!(
        check(&app).unwrap_err()[0]
            .message
            .contains("resolves outside")
    );
    let entry = scratch.write(
        "entry.rinzu",
        &format!(
            "import \"{}\" as value;fn main(){{let n=value.value();}}",
            outside.display()
        ),
    );
    assert!(check(&entry).is_ok());
    scratch.write(
        "native.rinzu",
        "native{fn raw()->CInt symbol \"provided\";}export raw;",
    );
    let entry = scratch.write(
        "entry.rinzu",
        "import \"./native.rinzu\" as nativeApi;fn main(){}",
    );
    let context = check(&entry).unwrap();
    let source = context.artifact.source_interface.unwrap();
    assert_eq!(source.signatures.len(), 1);
    assert!(source.signatures[0].name.ends_with("::raw"));
}

#[test]
fn syntax_and_utf8_errors_keep_the_target_position_and_full_import_chain() {
    let scratch = Scratch::new();
    let entry = scratch.write("main.rinzu", "import \"./a.rinzu\" as a;fn main(){}");
    scratch.write(
        "a.rinzu",
        "import \"./bad.rinzu\" as bad;fn value()->Int{return 1;}export value;",
    );
    let bad = scratch.0.join("bad.rinzu");
    // Invalid grammar/UTF-8 vectors deliberately bypass the formatter.
    for bytes in [
        b"fn broken( {".as_slice(),
        b"\xef\xbb\xbffn broken()->Int{return \xff;}",
    ] {
        std::fs::write(&bad, bytes).unwrap();
        let mut cache = SourceCache::default();
        let mut snapshot = SourceSnapshot::new(&mut cache, BTreeMap::new());
        let error = check_input_with_sources(
            &entry,
            &options(),
            &NativeCliInputs::default(),
            &mut snapshot,
        )
        .unwrap_err()
        .remove(0);
        assert_eq!(
            snapshot.path_for_id(error.primary.file_id),
            Some(bad.as_path())
        );
        assert_eq!(error.secondary.len(), 2);
        for path in [&entry, &scratch.0.join("a.rinzu"), &bad] {
            assert!(
                error.message.contains(&path.display().to_string()),
                "{error:?}"
            );
        }
    }
}
