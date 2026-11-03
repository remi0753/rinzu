use rinzu_compiler::{FileId, SourceFile, compile_source};

fn file(body: &str) -> SourceFile {
    SourceFile::new(
        FileId(1),
        "generic-shadowing.rinzu",
        format!("fn identity<T>(value: T) -> T {{ return value; }} {body}"),
    )
}

#[test]
fn local_functions_and_references_hide_generic_templates() {
    for body in [
        "fn main() { let identity = fn(value: Int) -> Int { return value + 1; }; let result = identity(7); }",
        "fn main() { let identity = fn(value: Int) -> Int { return value + 1; }; let alias = identity; let result = alias(7); }",
        "fn invoke(identity: fn(Int) -> Int) -> Int { return identity(7); } fn main() { let result = invoke(fn(value: Int) -> Int { return value; }); }",
        "fn main() { let invoke = fn(identity: fn(Int) -> Int) -> Int { return identity(7); }; let result = invoke(fn(value: Int) -> Int { return value; }); }",
    ] {
        compile_source(&file(body)).unwrap();
    }
}

#[test]
fn template_visibility_returns_after_blocks_and_other_function_bodies() {
    let artifact = compile_source(&file(
        "fn invoke(identity: fn(Int) -> Int) -> Int { return identity(7); }
         fn main() {
           { let identity = fn(value: Int) -> Int { return value + 1; }; let local = identity(7); }
           let value = identity<Int>(9);
         }",
    ))
    .unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|function| function.name == "identity<Int>")
    );
}

#[test]
fn method_receivers_can_hide_generic_type_names() {
    compile_source(&file(
        "class Box<T> { const value: T }
         class Plain { const value: Int fn get(self: Plain) -> Int { return self.value; } }
         fn main() { let Box = Plain(value: 7); let result = Box.get(); }",
    ))
    .unwrap();
}

#[test]
fn explicit_type_arguments_cannot_bypass_a_shadowing_local_value() {
    let diagnostics = compile_source(&file(
        "fn main() { let identity = fn(value: Int) -> Int { return value + 1; }; let result = identity<Int>(7); }"
    )).expect_err("type arguments must not select a hidden template");
    assert!(
        diagnostics.iter().any(
            |error| error.message.contains("local") && error.message.contains("type arguments")
        ),
        "{diagnostics:#?}"
    );
}

#[test]
fn case_and_catch_bindings_are_checked_as_local_call_targets() {
    for body in [
        "enum Item { Value(value: Int) } fn main() { switch Item.Value(value: 7) { case Item.Value(identity): { let result = identity(1); } } }",
        "error Stop {} fn main() { try { throw Stop(message: \"stop\"); } catch identity { let result = identity(1); } }",
    ] {
        let diagnostics = compile_source(&file(body)).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|error| error.message.contains("local call target")),
            "{diagnostics:#?}"
        );
    }
}
