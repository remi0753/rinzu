use rinzu_compiler::ast::{Declaration, ExpressionKind, Literal, StatementKind};
use rinzu_compiler::{
    DiagnosticCode, FileId, OperationKind, SourceFile, Type, compile_source, format_source,
    parse_source, verify_module,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "formatted.rinzu", text)
}

#[test]
fn parses_normal_expressions_nested_strings_and_preserves_literal_spelling() {
    let text =
        r#"fn main() { let value = f"前\{\}\u{0}{lookup("}") /* } */}{{"a": 1}}{f"内{2}"}"; }"#;
    let syntax = parse_source(&source(text)).unwrap();
    let Declaration::Function(function) = &syntax.declarations[0] else {
        panic!()
    };
    let StatementKind::Local { initializer, .. } = &function.body.statements[0].kind else {
        panic!()
    };
    let ExpressionKind::FormatString(parts) = &initializer.kind else {
        panic!()
    };
    assert_eq!(parts.len(), 4);
    assert_eq!(
        parts[0].kind,
        ExpressionKind::Literal(Literal::String("前{}\0".into()))
    );
    assert!(matches!(parts[1].kind, ExpressionKind::Call { .. }));
    assert!(matches!(parts[2].kind, ExpressionKind::Map(_)));
    assert!(matches!(parts[3].kind, ExpressionKind::FormatString(_)));
    let formatted = format_source(&source(text)).unwrap();
    assert!(formatted.contains(r#"f"前\{\}\u{0}{lookup("}") /* } */}{{"a": 1}}{f"内{2}"}"#));
    assert_eq!(format_source(&source(&formatted)).unwrap(), formatted);
}

#[test]
fn malformed_interpolation_and_unknown_names_keep_inner_original_spans() {
    for text in [
        r#"f"{}""#,
        r#"f"{ /* empty */ }""#,
        r#"f"{1 2}""#,
        r#"f"{1:2}""#,
        r#"f"{(1}""#,
        r#"f"{name""#,
        r#"f"}""#,
    ] {
        assert!(
            parse_source(&source(&format!("fn main() {{ let value = {text}; }}"))).is_err(),
            "{text}"
        );
    }
    let text = "fn main() { let value = f\"日本語{missing}\"; }\r\n";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(text.as_bytes());
    let file = SourceFile::from_utf8(FileId(7), "bom.rinzu", bytes).unwrap();
    let errors = compile_source(&file).unwrap_err();
    let error = errors
        .iter()
        .find(|error| error.code == DiagnosticCode::UnknownName)
        .unwrap();
    let start = text.find("missing").unwrap() + 3;
    assert_eq!(error.primary.start_byte as usize, start);
    assert_eq!(error.primary.end_byte as usize, start + "missing".len());
    assert_eq!(error.primary.file_id, FileId(7));
}

#[test]
fn creates_string_values_without_import_or_implicit_conversions() {
    let artifact = compile_source(&source(
        r#"
class Item { const text: String }
fn render(value: CSize) -> String { return f"数: {value} / {[1, 2]}"; }
fn main() { let empty = f""; let item = Item(text: f"{render(42)}"); let list = [f"{true}"]; }
"#,
    ))
    .unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .filter(|op| matches!(op.kind, OperationKind::FormatString { .. }))
            .all(|op| op.result.as_ref().unwrap().ty == Type::String)
    );
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "render")
            .unwrap()
            .facts
            .worker_safe
    );
    for text in [
        r#"fn main() { let value: Int = f"{1}"; }"#,
        r#"fn main() { let value = f"{[]}"; }"#,
        r#"fn main() { let value = f"{{}}"; }"#,
        r#"fn main() { let value: String = 42; }"#,
        r#"fn main() { let value = "\{"; }"#,
    ] {
        assert!(compile_source(&source(text)).is_err(), "{text}");
    }
}

#[test]
fn evaluation_order_and_async_effects_follow_embedded_expressions() {
    let artifact = compile_source(&source(
        r#"
fn first() -> Int { return 1; }
fn second() -> Int { return 2; }
fn main() { let value = f"{first()}:{second()}:{first()}"; }
"#,
    ))
    .unwrap();
    let main = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .unwrap();
    let calls: Vec<_> = main
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter_map(|op| {
            if let OperationKind::ManagedCall { target, .. } = &op.kind {
                Some(target.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(calls, ["first", "second", "first"]);
    let text = r#"async fn value() -> Int { return 1; }
async fn main() { let text = f"{await value()}"; }"#;
    compile_source(&source(text)).unwrap();
    assert!(
        compile_source(&source(&text.replace("async fn main", "fn main")))
            .unwrap_err()
            .iter()
            .any(|error| error.code == DiagnosticCode::InvalidAwait)
    );
}

#[test]
fn verifier_requires_matching_part_types_results_polls_and_roots() {
    let artifact = compile_source(&source(r#"fn main() { let value = f"前{"後"}"; }"#)).unwrap();
    for kind in 0..4 {
        let mut ir = artifact.ir.clone();
        let block = ir
            .functions
            .iter_mut()
            .find(|f| f.name == "main")
            .unwrap()
            .blocks
            .iter_mut()
            .find(|b| {
                b.operations
                    .iter()
                    .any(|op| matches!(op.kind, OperationKind::FormatString { .. }))
            })
            .unwrap();
        let index = block
            .operations
            .iter()
            .position(|op| matches!(op.kind, OperationKind::FormatString { .. }))
            .unwrap();
        match kind {
            0 => {
                let OperationKind::FormatString { parts, .. } = &mut block.operations[index].kind
                else {
                    panic!()
                };
                parts[0].1 = Type::Int;
            }
            1 => block.operations[index].result.as_mut().unwrap().ty = Type::Int,
            2 => {
                block.operations.remove(index - 1);
            }
            _ => {
                let OperationKind::GcSafepoint { stack_map, .. } =
                    &mut block.operations[index - 1].kind
                else {
                    panic!()
                };
                stack_map.roots.clear();
            }
        }
        assert!(verify_module(&ir).is_err(), "mutation {kind}");
    }
}

#[test]
fn resolves_module_constants_and_calls_inside_interpolation() {
    let root = std::env::temp_dir().join(format!("rinzu-formatted-package-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"formatted_app\"\nlanguage=\"0.2\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/amount.rinzu"),
        "let total = 42; fn label() -> String { return f\"{total}\"; } export total; export label;",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rinzu"),
        "import amount; fn main() { let text = f\"{amount.label()}:{amount.total}\"; }",
    )
    .unwrap();
    rinzu_compiler::check_package(&root).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
