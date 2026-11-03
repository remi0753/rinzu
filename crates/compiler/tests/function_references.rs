use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source};

fn compile(source: &str) -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(FileId(0), "references.rz", source)).unwrap()
}
#[test]
fn locals_assignments_parameters_and_async_references_keep_target_sets() {
    let artifact = compile(
        r#"
        fn add(x: Int) -> Int { return x + 1; }
        fn subtract(x: Int) -> Int { return x - 1; }
        fn apply(f: fn(Int) -> Int, x: Int) -> Int { return f(x); }
        async fn async_value(x: Int) -> Int { return x; }
        async fn main() {
            let direct = add;
            let alias: fn(Int) -> Int = direct;
            var selected = alias;
            selected = subtract;
            let first = selected(2);
            let second = apply(alias, first);
            let asynchronous: fn(Int) -> Future<Int> = async_value;
            let third = await asynchronous(second);
        }
    "#,
    );
    assert_eq!(artifact.checked.indirect_calls.len(), 3);
    assert!(
        artifact
            .checked
            .indirect_calls
            .values()
            .any(|targets| targets == &["async_value"])
    );
    assert!(artifact.checked.indirect_calls.values().any(|targets| {
        targets.iter().any(|t| t == "add") && targets.iter().any(|t| t == "subtract")
    }));
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|operation| matches!(operation.kind, OperationKind::IndirectCall { .. }))
    );
}
#[test]
fn aliases_work_as_native_callbacks_and_native_function_targets() {
    let artifact = compile(
        r#"
        native {
            fn invoke(f: fn(Int) -> Int) -> Int symbol "invoke";
            fn external(x: Int) -> Int symbol "external";
        }
        fn callback(x: Int) -> Int { return x; }
        fn main() {
            let alias = callback;
            let x = invoke(alias);
            let native_alias = external;
            let y = native_alias(x);
            let z = invoke(external);
        }
    "#,
    );
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "main")
            .unwrap()
            .facts
            .worker_safe
    );
    assert!(
        artifact
            .checked
            .indirect_calls
            .values()
            .any(|targets| targets == &["external"])
    );
}
#[test]
fn references_cannot_erase_signatures_or_cross_domains() {
    for body in [
        "let x = [identity];",
        "let x = {1: identity};",
        "let x = identity == identity;",
        "let x: fn(Bool) -> Int = identity;",
        "let x: supplant fn(Int) -> Int = identity;",
        "let x = spawn take(identity);",
        "let identity = 1; let x = identity(2);",
    ] {
        let source = format!(
            "fn identity(x: Int) -> Int {{ return x; }} fn take(f: fn(Int) -> Int) {{}} fn main() {{ {body} }}"
        );
        assert!(
            compile_source(&SourceFile::new(FileId(0), "references.rz", &source)).is_err(),
            "accepted {body}"
        );
    }
    for source in [
        "fn bad(x: List<fn(Int) -> Int>) {}",
        "fn bad(x: Future<fn(Int) -> Int>) {}",
    ] {
        let source = format!("fn identity(x: Int) -> Int {{ return x; }} {source} fn main() {{}}");
        assert!(
            compile_source(&SourceFile::new(FileId(0), "references.rz", &source)).is_err(),
            "accepted {source}"
        );
    }
    for source in [
        "fn make() -> fn(Int) -> Int { return identity; }",
        "class Command { const f: fn(Int) -> Int }",
        "enum Command { Value(f: fn(Int) -> Int) }",
    ] {
        compile(&format!(
            "fn identity(x:Int)->Int {{return x;}} {source} fn main() {{}}"
        ));
    }
}

#[test]
fn cleanup_effects_propagate_through_aliases_and_helpers() {
    let source = r#"
        async fn asynchronous() {}
        fn unsafe_cleanup() { let pending = asynchronous(); }
        fn wrapper() { unsafe_cleanup(); }
        fn main() { let cleanup = wrapper; try {} finally { cleanup(); } }
    "#;
    let errors = compile_source(&SourceFile::new(FileId(0), "references.rz", source)).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == rinzu_compiler::DiagnosticCode::InvalidFinally)
    );
    let artifact =
        compile("fn cleanup() {} fn main() { let alias = cleanup; try {} finally { alias(); } }");
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .all(|function| function.facts.cleanup_safe)
    );
}
