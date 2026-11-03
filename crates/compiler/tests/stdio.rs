use rinzu_compiler::{FileId, OperationKind, SourceFile, Type, compile_source, verify_module};

#[test]
fn stdout_has_unit_results_native_polls_and_error_routes() {
    let source = SourceFile::new(
        FileId(1),
        "stdio.rinzu",
        r#"
import rinzu.stdio;
fn worker(value: String) -> String { rinzu.stdio.write(value); return value; }
fn main() { try { rinzu.stdio.writeLine("日本語\u{0}🙂"); }
    catch [error: Error, ...errorRest] { let message = error.message; } finally { try { rinzu.stdio.write(""); } catch errors {} } }
"#,
    );
    let artifact = compile_source(&source).unwrap();
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "worker")
            .unwrap()
            .facts
            .worker_safe
    );
    let operations: Vec<_> = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter(|op| matches!(op.kind, OperationKind::Stdout { .. }))
        .collect();
    assert_eq!(operations.len(), 3);
    assert!(
        operations
            .iter()
            .all(|op| op.result.as_ref().unwrap().ty == Type::Unit)
    );
    let mut bad = artifact.ir.clone();
    let operation = bad
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
        .find(|op| matches!(op.kind, OperationKind::Stdout { .. }))
        .unwrap();
    operation.result.as_mut().unwrap().ty = Type::String;
    assert!(verify_module(&bad).is_err());
}

#[test]
fn stdout_requires_import_and_one_inferred_argument() {
    for body in [
        r#"fn main() { rinzu.stdio.write("x"); }"#,
        r#"import rinzu.stdio; fn main() { rinzu.stdio.write(); }"#,
        r#"import rinzu.stdio; fn main() { rinzu.stdio.write("x", "y"); }"#,
        r#"import rinzu.stdio; fn main() { rinzu.stdio.write([]); }"#,
        r#"import rinzu.stdio; fn main() { rinzu.stdio.write({}); }"#,
        r#"import rinzu.stdio; fn main() { let write = rinzu.stdio.write; }"#,
        r#"import rinzu.stdio; fn main() { let value: Int = rinzu.stdio.write("x"); }"#,
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(1), "bad.rinzu", body)).is_err(),
            "{body}"
        );
    }
}

#[test]
fn toolchain_stdio_resolves_without_dependency_and_cannot_be_replaced() {
    let root = std::env::temp_dir().join(format!("rinzu-stdio-module-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    let manifest = root.join("rinzu.toml");
    std::fs::write(
        &manifest,
        "[package]\nname=\"stdio_app\"\nlanguage=\"0.2\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        "import rinzu.stdio; fn main() { try { rinzu.stdio.writeLine(\"x\"); } catch errors {} }",
    )
    .unwrap();
    rinzu_compiler::check_package(&root).unwrap();
    std::fs::write(&manifest, "[package]\nname=\"stdio_app\"\nlanguage=\"0.2\"\n[dependencies]\n\"rinzu.stdio\"={path=\"../sdk\"}\n").unwrap();
    assert!(rinzu_compiler::check_package(&root).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
