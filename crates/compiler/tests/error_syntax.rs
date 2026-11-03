use std::collections::BTreeMap;

use rinzu_compiler::ast::{CatchPattern, Declaration, ExpressionKind, StatementKind, TypeRefKind};
use rinzu_compiler::{
    DiagnosticCode, FileId, SourceFile, SourceRange, build_editor_index, compile_source,
    format_source, parse_source, source_documentation,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(12), "errors.rinzu", text)
}

#[test]
fn errors_are_distinct_declarations_with_generic_and_optional_payloads() {
    let syntax = parse_source(&source(
        "error Missing {}\nerror Invalid<T> { payload?: { field: String, value: T } }\nerror Batch { payload: List<Error>, }",
    ))
    .unwrap();
    let Declaration::Error(missing) = &syntax.declarations[0] else {
        panic!("Error must not become a class or enum")
    };
    assert_eq!(missing.name, "Missing");
    assert!(missing.payload.is_none());
    let Declaration::Error(invalid) = &syntax.declarations[1] else {
        panic!()
    };
    assert_eq!(invalid.type_parameters[0].name, "T");
    let payload = invalid.payload.as_ref().unwrap();
    assert!(payload.optional);
    let TypeRefKind::Record(fields) = &payload.ty.kind else {
        panic!()
    };
    assert_eq!(fields[1].name, "value");
    assert_eq!(fields[1].ty.kind, TypeRefKind::Named(vec!["T".into()]));
    let Declaration::Error(batch) = &syntax.declarations[2] else {
        panic!()
    };
    assert!(!batch.payload.as_ref().unwrap().optional);
    assert!(matches!(
        batch.payload.as_ref().unwrap().ty.kind,
        TypeRefKind::List(_)
    ));
}

#[test]
fn error_remains_contextual_so_the_existing_error_constructor_is_not_a_keyword() {
    let syntax = parse_source(&source(
        "fn main() { let value = error(1, \"old constructor\"); throw value; }",
    ))
    .unwrap();
    let Declaration::Function(function) = &syntax.declarations[0] else {
        panic!()
    };
    let StatementKind::Local { initializer, .. } = &function.body.statements[0].kind else {
        panic!()
    };
    assert!(
        matches!(&initializer.kind, ExpressionKind::Call { target, .. } if target == &["error"])
    );
    assert!(matches!(
        function.body.statements[1].kind,
        StatementKind::Throw(_)
    ));
}

#[test]
fn catches_preserve_order_typed_bindings_rest_and_finally() {
    let syntax = parse_source(&source(
        "fn main() { try { throw Invalid<Int>(message: \"invalid\"); }\ncatch [problem: Invalid<Int>, ...rest] {}\ncatch [Missing] {}\ncatch errors {} finally {} }",
    ))
    .unwrap();
    let Declaration::Function(function) = &syntax.declarations[0] else {
        panic!()
    };
    let StatementKind::TryErrors {
        all,
        body,
        handlers,
        cleanup,
    } = &function.body.statements[0].kind
    else {
        panic!()
    };
    assert!(!all);
    assert!(matches!(body.statements[0].kind, StatementKind::Throw(_)));
    assert_eq!(handlers.len(), 3);
    assert!(cleanup.is_some());
    let CatchPattern::List { elements, rest, .. } = &handlers[0].pattern else {
        panic!()
    };
    assert_eq!(elements[0].binding.as_ref().unwrap().name, "problem");
    assert!(
        matches!(&elements[0].ty.kind, TypeRefKind::Generic { path, .. } if path == &["Invalid"])
    );
    assert_eq!(rest.as_ref().unwrap().name, "rest");
    let CatchPattern::List { elements, rest, .. } = &handlers[1].pattern else {
        panic!()
    };
    assert!(elements[0].binding.is_none());
    assert!(rest.is_none());
    assert!(
        matches!(&handlers[2].pattern, CatchPattern::Binding(binding) if binding.name == "errors")
    );
}

#[test]
fn try_all_accepts_the_same_patterns_and_preserves_finally_without_a_catch() {
    let syntax = parse_source(&source(
        "fn main() { try all { throw A(message: \"a\"); } catch [a: A] {} try all { throw A(message: \"a\"); } finally {} }",
    )).unwrap();
    let Declaration::Function(function) = &syntax.declarations[0] else {
        panic!()
    };
    assert!(matches!(
        &function.body.statements[0].kind,
        StatementKind::TryErrors { all: true, .. }
    ));
    assert!(matches!(
        &function.body.statements[1].kind,
        StatementKind::TryAll {
            handler: None,
            cleanup: Some(_),
            ..
        }
    ));
}

#[test]
fn invalid_error_declarations_and_rest_positions_are_diagnosed() {
    for text in [
        "error Missing { message: String }",
        "error Invalid { payload: Int, payload: String }",
        "error Invalid { payload? Int }",
        "error Invalid { payload: }",
        "error Invalid<T> { payload: T; }",
        "fn main() { throw; }",
        "fn main() { throw A(message: \"a\") }",
        "fn main() { try {} catch [a: A, ...rest, b: B] {} }",
        "fn main() { try {} catch [a: A, ...rest, ...other] {} }",
        "fn main() { try {} catch [a:] {} }",
        "fn main() { try all {} }",
    ] {
        assert!(parse_source(&source(text)).is_err(), "{text}");
    }
}

#[test]
fn empty_catch_is_rejected_at_the_original_pattern_span() {
    let text = "fn main() { try {} catch [] {} }";
    let diagnostics = parse_source(&source(text)).unwrap_err();
    assert_eq!(diagnostics[0].code, DiagnosticCode::InvalidCatchPattern);
    let span = diagnostics[0].primary;
    assert_eq!(
        &text[span.start_byte as usize..span.end_byte as usize],
        "[]"
    );
}

#[test]
fn statement_all_and_typed_catch_use_inferred_exhaustiveness() {
    compile_source(&source("fn main() { try all {} catch [e: Error] {} }")).unwrap();
    compile_source(&source("fn main() { try {} catch [e: Error] {} }")).unwrap();
    let artifact = compile_source(&source("error Missing {} fn fail() { throw Missing(message: \"a\"); } fn main() { try {fail();} catch [problem: Missing] {} catch errors {} }")).unwrap();
    let operations: Vec<_> = artifact
        .ir
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
        .collect();
    assert!(operations.iter().any(|operation| matches!(
        operation.kind,
        rinzu_compiler::OperationKind::ThrowError { .. }
    )));
    assert!(operations.iter().any(|operation| matches!(
        operation.kind,
        rinzu_compiler::OperationKind::CatchErrors { .. }
    )));
}

#[test]
fn formatter_preserves_error_payload_patterns_and_nested_type_arguments() {
    let input = source(
        "/// validation error\nerror Invalid<T>{payload?:{field:String,value:T}}\nfn main(){try all{throw Invalid<List<Int>>(message:\"日本語\",payload:{field=\"x\",value=[1]});}catch [problem:Invalid<List<Int>>,...rest]{use(problem);use(rest);}catch errors{}finally{cleanup();}}",
    );
    let formatted = format_source(&input).unwrap();
    assert!(formatted.contains("error Invalid<T>"), "{formatted}");
    assert!(formatted.contains("payload?:"), "{formatted}");
    assert!(formatted.contains("Invalid<List<Int>>"), "{formatted}");
    assert!(formatted.contains("...rest"), "{formatted}");
    assert!(formatted.contains("/// validation error"), "{formatted}");
    assert!(formatted.contains("日本語"), "{formatted}");
    assert_eq!(format_source(&source(&formatted)).unwrap(), formatted);
    assert_eq!(
        parse_source(&source(&formatted))
            .unwrap()
            .declarations
            .len(),
        2
    );
}

#[test]
fn syntax_spans_keep_bom_crlf_and_unicode_offsets() {
    let text = "/// 日本語\r\nerror Failure { payload?: String }\r\nfn main() { try {} catch [problem: Failure, ...rest] { throw problem; } }";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(text.as_bytes());
    let file = SourceFile::from_utf8(FileId(19), "unicode.rinzu", bytes.clone()).unwrap();
    let syntax = parse_source(&file).unwrap();
    let Declaration::Error(error) = &syntax.declarations[0] else {
        panic!()
    };
    let slice = |range: SourceRange| {
        std::str::from_utf8(&bytes[range.start_byte as usize..range.end_byte as usize]).unwrap()
    };
    assert_eq!(slice(error.range), "error Failure { payload?: String }");
    assert_eq!(
        slice(error.payload.as_ref().unwrap().range),
        "payload?: String"
    );
    let Declaration::Function(function) = &syntax.declarations[1] else {
        panic!()
    };
    let StatementKind::TryErrors { handlers, .. } = &function.body.statements[0].kind else {
        panic!()
    };
    let bindings: Vec<_> = handlers[0]
        .pattern
        .bindings()
        .map(|binding| slice(binding.range))
        .collect();
    assert_eq!(bindings, ["problem", "rest"]);
}

#[test]
fn documentation_and_navigation_keep_error_identity_and_catch_local_scopes() {
    let input = source(
        "/// A failure\nerror Failure<T> {\n    /// Additional context\n    payload?: T\n}\nfn main() { try {} catch [problem: Failure<Int>, ...rest] { use(problem); use(rest); } catch errors { use(errors); } }\n",
    );
    let syntax = parse_source(&input).unwrap();
    let documentation = source_documentation(&input).unwrap();
    assert_eq!(documentation[0].kind, "error");
    assert_eq!(documentation[0].documentation, "A failure");
    assert_eq!(
        documentation[0].members[0].documentation,
        "Additional context"
    );
    assert!(
        documentation[0].members[0]
            .signature
            .starts_with("payload?:")
    );
    let sources = BTreeMap::from([(input.path().to_path_buf(), input.clone())]);
    let index = build_editor_index(&sources, &[(input.clone(), syntax)], &[]);
    assert!(
        index
            .symbols
            .iter()
            .any(|symbol| symbol.name == "Failure" && symbol.kind == "error")
    );
    for name in ["problem", "rest", "errors"] {
        let symbol = index
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap();
        let call = format!("use({name})");
        let offset = input.text().find(&call).unwrap() + 4;
        let position = SourceRange::new(input.id(), offset, offset + name.len());
        assert_eq!(index.definition_at(position), Some(symbol.definition));
    }
    let end = input.text().len();
    let visible = index.visible_symbols(SourceRange::new(input.id(), end, end));
    assert!(
        !visible
            .iter()
            .any(|symbol| matches!(symbol.name.as_str(), "problem" | "rest" | "errors"))
    );
    assert!(visible.iter().any(|symbol| symbol.name == "Failure"));
}
