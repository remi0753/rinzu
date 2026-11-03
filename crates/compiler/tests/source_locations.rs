use rinzu_compiler::{
    CompilationOptions, FileId, NativeCliInputs, OperationKind, SourceCache, SourceFile,
    SourceSnapshot, check_input, check_input_with_sources, compile_source,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rinzu-source-locations-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn write(&self, path: &str, text: &str) -> PathBuf {
        let path = self.0.join(path);
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn options() -> CompilationOptions {
    CompilationOptions {
        language: None,
        target: "aarch64-apple-darwin".into(),
    }
}
fn check(path: &Path) -> rinzu_compiler::CompilationContext {
    check_input(path, &options(), &NativeCliInputs::default()).unwrap()
}
fn first_throw(ir: &rinzu_compiler::IrModule) -> rinzu_compiler::SourceRange {
    ir.functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .find(|op| matches!(op.kind, OperationKind::ThrowError { .. }))
        .unwrap()
        .range
}

#[test]
fn coordinates_use_one_based_unicode_scalars_tabs_crlf_and_original_bom_offsets() {
    let text = "error A{}\r\nfn fail(){let label=\"界🙂\";\tthrow A(message:\"x\");}\r\nfn main(){try{fail();}catch[A]{}}";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(text.as_bytes());
    let file = SourceFile::from_utf8(FileId(201), "unicode.rinzu", bytes).unwrap();
    let artifact = compile_source(&file).unwrap();
    let range = first_throw(&artifact.ir);
    assert_eq!(range.start_byte as usize, text.find("throw").unwrap() + 3);
    let at = artifact.ir.source_locations[&201][&range.start_byte];
    assert_eq!((at.line, at.column), (2, 27));
    for (&offset, at) in &artifact.ir.source_locations[&201] {
        assert_eq!(
            (at.line as usize, at.column as usize),
            file.line_column(offset as usize)
        );
    }
    assert!(artifact.ir.source_locations[&201].len() < text.chars().count());
}

#[test]
fn generic_error_operations_keep_the_original_file_coordinates() {
    let file = SourceFile::new(
        FileId(202),
        "generic.rinzu",
        "error Invalid<T>{payload:T}\nfn fail<T>(value:T){throw Invalid<T>(message:\"x\",payload:value);}\nfn main(){try{fail<Int>(7);}catch[Invalid<Int>]{}}",
    );
    let artifact = compile_source(&file).unwrap();
    let range = first_throw(&artifact.ir);
    assert_eq!(range.file_id, file.id());
    let at = artifact.ir.source_locations[&202][&range.start_byte];
    assert_eq!((at.line, at.column), (2, 21));
}

#[test]
fn file_and_package_imports_embed_coordinates_before_source_files_are_removed() {
    for package in [false, true] {
        let scratch = Scratch::new();
        scratch.write(
            "src/model.rinzu",
            "error A{}\nfn fail(){throw A(message:\"x\");}export fail;",
        );
        let entry = scratch.write(
            "src/main.rinzu",
            "import \"./model.rinzu\" as model;\nfn main(){try{model.fail();}catch errors{}}",
        );
        if package {
            scratch.write(
                "rinzu.toml",
                "[package]\nname=\"locations\"\nlanguage=\"0.5\"\n",
            );
        }
        let context = check(if package { &scratch.0 } else { &entry });
        let range = first_throw(&context.artifact.ir);
        std::fs::remove_file(entry).unwrap();
        std::fs::remove_file(scratch.0.join("src/model.rinzu")).unwrap();
        let at = context.artifact.ir.source_locations[&range.file_id.0][&range.start_byte];
        assert_eq!((at.line, at.column), (2, 11));
        assert!(context.artifact.ir.source_files[&range.file_id.0].ends_with("model.rinzu"));
    }
}

#[test]
fn editor_overlay_coordinates_come_from_the_checked_generation() {
    let scratch = Scratch::new();
    let entry = scratch.write("main.rinzu", "fn main(){}\n");
    let overlay = "\n\nerror A{}\nfn probe(){\nlet text=\"🙂\";throw A(message:\"overlay\");\n}fn main(){try{probe();}catch[A]{}}";
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(
        &mut cache,
        BTreeMap::from([(entry.clone(), overlay.as_bytes().to_vec())]),
    );
    let context = check_input_with_sources(
        &entry,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .unwrap();
    let range = first_throw(&context.artifact.ir);
    let at = context.artifact.ir.source_locations[&range.file_id.0][&range.start_byte];
    assert_eq!((at.line, at.column), (5, 14));
    assert_eq!(std::fs::read_to_string(entry).unwrap(), "fn main(){}\n");
}

#[test]
fn standard_source_interfaces_keep_their_dependency_coordinates() {
    let scratch = Scratch::new();
    let entry = scratch.write("main.rinzu", "import rinzu.io;async fn main(){try{let pending=rinzu.io.closeOwnerFiles();let closed=await pending;}catch errors{}}");
    let context = check(&entry);
    let mut checked = 0;
    for package in &context.artifact.sdk_packages {
        if let Some(source) = &package.source {
            assert!(
                !source.source_locations.is_empty(),
                "{}",
                package.manifest.name
            );
            for (file, positions) in &source.source_locations {
                assert!(source.source_files.contains_key(file));
                for (offset, at) in positions {
                    assert_eq!(context.artifact.ir.source_locations[file][offset], *at);
                    assert!(at.line > 0 && at.column > 0);
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0);
}
