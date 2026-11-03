use rinzu_compiler::{FileId, SourceFile, compile_source};

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "lambda-constants.rinzu", text)
}

#[test]
fn lambda_parameters_cannot_be_used_as_generic_byte_lengths() {
    for source in [
        "let size = 4; fn main() { let f = fn(size: Int) { let x = id<Bytes(size)>(b\"\"); }; f(8); }",
        "fn main() { let size = 4; let f = fn(size: Int) { let x = id<Bytes(size)>(b\"\"); }; f(8); }",
        "let size = 4; fn main() { let f = fn(size: Int) { let inner = fn() { let x = id<Bytes(size)>(b\"\"); }; inner(); }; f(8); }",
    ] {
        let source = format!("fn id<T>(value: T) -> T {{ return value; }} {source}");
        let errors = compile_source(&file(&source))
            .expect_err("runtime lambda parameter must hide the outer constant");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("constant")),
            "{errors:#?}"
        );
    }
}

#[test]
fn lambda_parameters_hide_constants_in_all_signature_types() {
    for source in [
        "let size = 4; fn main() { let f = fn(size: Int) -> Bytes(size) { return b\"\"; }; }",
        "let size = 4; fn main() { let f = fn(size: Int, bytes: Bytes(size)) {}; }",
        "let size = 4; fn main() { let f = fn(bytes: Bytes(size), size: Int) {}; }",
        "fn main() { let size = 4; let f = fn(size: Int) -> Bytes(size) { return b\"\"; }; }",
    ] {
        let errors = compile_source(&file(source))
            .expect_err("runtime lambda parameter must not supply a signature constant");
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("constant")),
            "{source}: {errors:#?}"
        );
    }
}

#[test]
fn parameter_shadowing_does_not_change_constants_outside_the_lambda() {
    for declaration in ["let size = 4; fn main() {", "fn main() { let size = 4;"] {
        let source = format!(
            "fn id<T>(value: T) -> T {{ return value; }} {declaration}
             let echo = fn(size: Int) -> Int {{ return size; }};
             let value: Bytes(4) = id<Bytes(size)>(b\"ab\");
             let read = fn() -> Bytes(size) {{ return b\"cd\"; }};
             let bytes: Bytes(4) = read();
             let result = echo(8);
             }}"
        );
        let artifact = compile_source(&file(&source)).unwrap();
        assert!(
            artifact
                .ir
                .functions
                .iter()
                .any(|function| function.name == "id<Bytes(4)>")
        );
    }
}
