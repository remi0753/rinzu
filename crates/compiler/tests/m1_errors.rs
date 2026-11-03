use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};
fn file(s: &str) -> SourceFile {
    SourceFile::new(FileId(1), "errors.rinzu", s)
}
#[test]
fn error_values_and_causes_remain_usable_in_the_same_domain() {
    let source = file(include_str!(
        "../../../tests/fixtures/programs/errors/src/main.rinzu"
    ));
    let formatted = format_source(&source).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file(&formatted)).unwrap();
    compile_source(&file("fn wrap(value: Error)->Error {return errorWithCause(value,error(1000,\"cause\"));} fn main() {}" )).unwrap();
}
#[test]
fn errors_and_aggregates_containing_errors_cannot_cross_workers() {
    for text in [
        "fn echo(value: Error)->Error{return value;} async fn main(){let pending=spawn echo(error(1,\"x\"));let value=await pending;}",
        "fn create()->Error{return error(1,\"x\");} async fn main(){let pending=spawn create();let value=await pending;}",
        "fn echo(value: List<Error>)->Unit{} async fn main(){let pending=spawn echo([error(1,\"x\")]);await pending;}",
        "enum Result<T,E>{Ok(value:T),Err(error:E)} fn echo<T: Transferable>(value: T)->T{return value;} fn main(){let value=echo<Result<Error,Error>>(Result<Error,Error>.Err(error:error(1,\"x\")));}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}
#[test]
fn errors_reject_equality_and_wrong_metadata_types() {
    for s in [
        "fn main() {let x=error(1000,\"a\")==error(1000,\"a\");}",
        "fn main(){let x=error(\"code\",\"a\");}",
        "fn main(){let x=errorWithCause(error(1000,\"a\"),1);}",
    ] {
        assert!(compile_source(&file(s)).is_err(), "{s}");
    }
}
