use rinzu_compiler::{
    CheckedModule, CompilerOptions, DiagnosticCode, ExternalType, FileId, SourceFile, Type,
    check_module, check_module_with_options, compile_source, parse_source,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(21), "error-types.rinzu", text)
}

fn check(text: &str) -> CheckedModule {
    check_module(&parse_source(&source(text)).unwrap())
        .unwrap_or_else(|errors| panic!("{text}\n{errors:#?}"))
}

#[test]
fn dedicated_errors_have_readonly_standard_fields_and_typed_payloads() {
    let checked = check(
        r#"
class Document { value: Int }
error Missing {}
error SaveFailed { payload: Document }
error Invalid<T> { payload?: T }
fn read(error: SaveFailed) -> Document { return error.payload; }
fn optional(error: Invalid<Int>) -> Option<Int> { return error.payload; }
fn message(error: Error) -> String { return error.message; }
fn name(error: Error) -> String { return error.name; }
fn cause(error: Error) -> Option<Error> { return error.cause; }
fn source(error: Error) -> Option<SourceLocation> { return error.source; }
fn main() {
    let first = Missing(message: "missing");
    let document = Document(value: 1);
    let second = SaveFailed(message: "save", payload: document, cause: first);
    let empty = Invalid<Int>(message: "optional");
    let present = Invalid<Int>(message: "optional", payload: 42);
}
"#,
    );
    for name in ["Missing", "SaveFailed", "Invalid<Int>"] {
        let schema = &checked.source_types[name];
        assert_eq!(schema.kind, "error");
        assert_eq!(schema.const_fields.len(), schema.fields.len());
        assert!(schema.variants.is_empty());
        assert_eq!(schema.fields[0], ("name".into(), Type::String));
        assert_eq!(schema.fields[1], ("message".into(), Type::String));
    }
    assert!(
        checked.source_types["Missing"]
            .fields
            .iter()
            .all(|(name, _)| name != "payload")
    );
    assert_eq!(
        checked.source_types["SaveFailed"].fields.last().unwrap().1,
        Type::Named("Document".into())
    );
    assert_eq!(
        checked.source_types["Invalid<Int>"]
            .fields
            .last()
            .unwrap()
            .1,
        Type::Named("Option<Int>".into())
    );
}

#[test]
fn constructors_require_message_and_only_the_declared_payload() {
    for (declaration, expression, message) in [
        ("error E {}", "E()", "named arguments"),
        (
            "error E {}",
            "E(payload: 1)",
            "missing the required message",
        ),
        ("error E {}", "E(message: 1)", "expected `String`"),
        (
            "error E {}",
            "E(message: \"x\", name: \"fake\")",
            "no argument `name`",
        ),
        (
            "error E {}",
            "E(message: \"x\", source: 1)",
            "no argument `source`",
        ),
        (
            "error E {}",
            "E(message: \"x\", payload: 1)",
            "no argument `payload`",
        ),
        (
            "error E {}",
            "E(message: \"x\", cause: 1)",
            "expected `Error`",
        ),
        (
            "error E { payload: Int }",
            "E(message: \"x\")",
            "required payload",
        ),
        (
            "error E { payload: Int }",
            "E(message: \"x\", payload: \"wrong\")",
            "expected `Int`",
        ),
        (
            "error E { payload?: Int }",
            "E(message: \"x\", payload: \"wrong\")",
            "expected `Int`",
        ),
        (
            "error E {}",
            "E(message: \"x\", message: \"y\")",
            "more than once",
        ),
    ] {
        let text = format!("{declaration} fn main() {{ let value = {expression}; }}");
        let errors = check_module(&parse_source(&source(&text)).unwrap()).unwrap_err();
        assert!(
            errors.iter().any(|error| error.message.contains(message)),
            "{text}: {errors:#?}"
        );
    }
}

#[test]
fn generic_identity_and_aliases_use_the_existing_specialization_rules() {
    let checked = check(
        "type T = String; type Number = Int; error Context<T> { payload: T } fn id(value: Context<Number>) -> Context<Int> { return value; } fn main() { let first = Context<Int>(message: \"x\", payload: 1); let second = Context<String>(message: \"x\", payload: \"y\"); }",
    );
    assert_eq!(
        checked.source_types["Context<Int>"]
            .fields
            .last()
            .unwrap()
            .1,
        Type::Int
    );
    assert_eq!(
        checked.source_types["Context<String>"]
            .fields
            .last()
            .unwrap()
            .1,
        Type::String
    );
    assert!(!checked.source_types.contains_key("Context<Number>"));
    let invalid = source(
        "error Context<T> { payload: T } fn wrong(value: Context<Int>) -> Context<String> { return value; } fn main() {}",
    );
    assert!(
        check_module(&parse_source(&invalid).unwrap())
            .unwrap_err()
            .iter()
            .any(|error| error.code == DiagnosticCode::TypeMismatch)
    );
}

#[test]
fn errors_can_be_upcast_but_classes_cannot_impersonate_errors() {
    check(
        "error E {} fn convert() -> Error { return E(message: \"x\"); } fn use(value: Error) {} fn main() { let common: Error = E(message: \"x\"); use(E(message: \"y\")); }",
    );
    let text = "class Pretend { message: String } fn convert(value: Pretend) -> Error { return value; } fn main() {}";
    assert!(
        check_module(&parse_source(&source(text)).unwrap())
            .unwrap_err()
            .iter()
            .any(|error| error.code == DiagnosticCode::TypeMismatch)
    );
}

#[test]
fn payloads_follow_ordinary_same_domain_reference_semantics() {
    check(
        "class Document { version: Int } error E { payload: Document } fn update(error: E) { error.payload.version = 2; } fn main() { let document = Document(version: 1); let value = E(message: \"x\", payload: document); document.version = 3; }",
    );
    for assignment in ["value.message = \"y\";", "value.payload = 2;"] {
        let text = format!(
            "error E {{ payload: Int }} fn main() {{ let value = E(message: \"x\", payload: 1); {assignment} }}"
        );
        assert!(
            check_module(&parse_source(&source(&text)).unwrap()).is_err(),
            "{text}"
        );
    }
}

#[test]
fn future_payloads_are_rejected_through_nested_and_cyclic_type_graphs() {
    for text in [
        "error E { payload: Future<Int> } fn main() {}",
        "error E { payload?: List<Future<Int>> } fn main() {}",
        "error E<T> { payload: T } fn accept(value: E<Future<Int>>) {} fn main() {}",
        "type Wrapped = { jobs: Map<String, Future<Int>> }; error E { payload: Wrapped } fn main() {}",
        "class Node { next: Option<Node>, job: Future<Int> } error E { payload: Node } fn main() {}",
        "enum Job { Pending(value: Future<Int>), Done } error E { payload: Job } fn main() {}",
        "error Inner { payload: Future<Int> } error Outer { payload: Inner } fn main() {}",
    ] {
        let errors = check_module(&parse_source(&source(text)).unwrap()).unwrap_err();
        assert!(
            errors.iter().any(|error| error
                .message
                .contains("Error payload cannot contain Future")),
            "{text}: {errors:#?}"
        );
    }
    check("class Node { next: Option<Node>, value: Int } error E { payload: Node } fn main() {}");
}

#[test]
fn a_function_can_produce_future_without_storing_a_future_in_the_payload() {
    check(
        "error E { payload: fn()->Future<Int> } fn main() { let value = E(message: \"x\", payload: async fn()->Int { return 42; }); }",
    );
    let text = "error E { payload: fn()->Future<Int> } async fn work()->Int { return 1; } async fn main() { let pending = work(); let value = E(message: \"x\", payload: async fn()->Int { return await pending; }); }";
    assert!(check_module(&parse_source(&source(text)).unwrap()).is_err());
}

#[test]
fn call_only_values_cannot_escape_through_error_payloads() {
    let syntax = parse_source(&source("error E { payload: Borrowed } fn main() {}")).unwrap();
    let mut options = CompilerOptions::default();
    options
        .external_types
        .insert("Borrowed".into(), ExternalType { call_only: true });
    let errors = check_module_with_options(&syntax, &options).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::CallOnlyEscape)
    );
}

#[test]
fn source_location_fields_are_typed_readonly_and_publicly_spelled() {
    let checked = check(
        "fn get(value: Error) -> Int { switch value.source { case Option.Some(position): { return position.line; } case Option.None: { return 0; } } } fn main() {}",
    );
    let source_type = checked
        .source_types
        .values()
        .find(|schema| {
            schema
                .fields
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                == ["file", "line", "column"]
        })
        .unwrap();
    assert_eq!(source_type.const_fields, [0, 1, 2]);
    let function = checked
        .functions
        .iter()
        .find(|function| function.declaration.name == "get")
        .unwrap();
    assert_eq!(function.result_type, Type::Int);
    assert_eq!(
        Type::Named("$rinzu_SourceLocation".into()).to_string(),
        "SourceLocation"
    );
    let text = "fn update(position: SourceLocation) { position.line = 0; } fn main() {}";
    assert!(check_module(&parse_source(&source(text)).unwrap()).is_err());
}

#[test]
fn error_values_and_aggregates_do_not_have_equality_key_or_transferable_markers() {
    for (definition, argument) in [("error E {}", "E(message: \"x\")"), ("", "error(1, \"x\")")] {
        for marker in ["Equality", "Key", "Transferable"] {
            let text = format!(
                "{definition} fn require<T: {marker}>(value: T) {{}} fn main() {{ require<Error>({argument}); }}"
            );
            assert!(
                check_module(&parse_source(&source(&text)).unwrap()).is_err(),
                "{text}"
            );
        }
    }
    let text = "fn use(value: List<Error>) {} async fn main() { let pending = spawn use([error(1, \"x\")]); await pending; }";
    assert!(check_module(&parse_source(&source(text)).unwrap()).is_err());
}

#[test]
fn typed_error_constructors_and_metadata_lower_to_verified_ir() {
    for text in [
        "error E {} fn main() { let value = E(message: \"x\"); }",
        "fn main() { let value = error(1, \"x\"); let name = value.name; }",
        "fn main() { let value = error(1, \"x\"); let cause = value.cause; }",
    ] {
        check(text);
        compile_source(&source(text)).unwrap();
    }
}
