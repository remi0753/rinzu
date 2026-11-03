use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};

fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId(1), "results.rinzu", source)
}

#[test]
fn user_results_specialize_and_case_names_are_local() {
    let source = file(
        r#"
enum Result<T,E>{Ok(value:T),Err(error:E)}
fn main() {
    let nested = Option<Result<Int, String>>.Some(value: Result<Int, String>.Ok(value: 42));
    switch nested {
        case Option.Some(value): {
            switch value {
                case Result.Ok(number): {}
                case Result.Err(number): {}
            }
        }
        case Option.None: {}
    }
    switch Result<Int, String>.Err(error: "bad") {
        case Result.Ok(value): {}
        case Result.Err(value): {}
    }
}
"#,
    );
    let once = format_source(&source).unwrap();
    assert_eq!(once, format_source(&file(&once)).unwrap());
    let artifact = compile_source(&file(&once)).unwrap();
    assert!(
        artifact
            .ir
            .source_types
            .contains_key("Option<Result<Int,String>>")
    );
    assert!(artifact.ir.source_types.contains_key("Result<Int,String>"));
}

#[test]
fn option_variants_payload_scope_and_properties_are_checked() {
    for source in [
        "fn main() { switch Option<Int>.None() { case Option.None: {} } }",
        "fn main() { switch Option<Int>.None() { case Option.None: {} case Option.Some: {} } }",
        "fn main() { switch Option<Int>.None() { case Option.None: {} case Option.Some(value): {} } let x = value; }",
        "fn main() { let value = 1; switch Option<Int>.None() { case Option.None: {} case Option.Some(value): {} } }",
        "fn require<T: Equality>(value: T) {} fn bad(value: Option<Future<Unit>>) { require<Option<Future<Unit>>>(value); } fn main() {}",
        "fn work(value: Option<Future<Unit>>) {} async fn child() {} async fn main() { let x = Option<Future<Unit>>.Some(value: child()); let y = spawn work(x); await y; }",
    ] {
        assert!(compile_source(&file(source)).is_err(), "{source}");
    }
    compile_source(&file(
        "enum Result { Good } fn main() { let value = Result.Good(); }",
    ))
    .unwrap();
    compile_source(&file("fn require<T: Transferable + Equality + Key>(value: T) {} fn main() { require<Option<Int>>(Option<Int>.Some(value: 42)); }")).unwrap();
}
