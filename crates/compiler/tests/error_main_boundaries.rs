use rinzu_compiler::{
    CompilerOptions, DiagnosticCode, FileId, SourceFile, compile_source,
    compile_source_with_options,
};
const BASE: &str = "error Failed{}error Cleanup{}fn fail(){throw Failed(message:\"failure\");}async fn future(){fail();}";
fn check(text: &str, valid: bool) {
    let result = compile_source(&SourceFile::new(
        FileId(279),
        "main-errors.rinzu",
        format!("{BASE}{text}"),
    ));
    match result {
        Ok(_) => assert!(valid, "an Error escaped Main"),
        Err(errors) => {
            assert!(!valid, "{errors:#?}");
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary
                        && error.message.contains("Main")),
                "{errors:#?}"
            );
        }
    }
}
#[test]
fn synchronous_and_async_main_must_catch_normal_errors() {
    check("fn main(){fail();}", false);
    check("async fn main(){fail();}", false);
    check("fn main(){try{fail();}catch[Failed]{}}", true);
    check("async fn main(){try{await future();}catch[Failed]{}}", true);
    check("async fn main(){await future();}", false);
}
#[test]
fn catch_and_finally_can_still_leave_main_with_an_error() {
    check(
        "fn main(){try{fail();}catch[Failed]{throw Cleanup(message:\"handler\");}}",
        false,
    );
    check("fn main(){try{}finally{fail();}}", false);
    check(
        "fn main(){try{try{fail();}finally{throw Cleanup(message:\"cleanup\");}}catch[Failed,Cleanup]{}}",
        true,
    );
}
#[test]
fn known_conditions_do_not_invent_unreachable_errors() {
    check(
        "fn choose(flag:Bool){if flag{fail();}}fn main(){choose(false);}",
        true,
    );
    check(
        "fn choose(flag:Bool){if flag{fail();}}fn main(){choose(true);}",
        false,
    );
    check("fn main(){if false{fail();}}", true);
}
#[test]
fn native_result_preparation_and_logging_are_checked() {
    check(
        "native{fn get()->String symbol \"get\";}fn main(){let text=get();}",
        false,
    );
    check(
        "native{fn get()->String symbol \"get\";}fn main(){try{let text=get();}catch errors{}}",
        true,
    );
    let text = "import rinzu.stdio;fn main(){rinzu.stdio.writeLine(7);}";
    assert!(
        compile_source(&SourceFile::new(FileId(279), "main-log.rinzu", text))
            .unwrap_err()
            .iter()
            .any(|error| error.code == DiagnosticCode::ErrorEscapesBoundary)
    );
}
#[test]
fn ordinary_errors_and_traps_keep_different_boundaries() {
    check("fn main(){let trap=1/0;}", true);
    check("fn main(){let invalid=CInt8(300);}", false);
    check(
        "fn main(){try{let invalid=CInt8(300);}catch errors{}}",
        true,
    );
}
#[test]
fn library_functions_named_main_are_not_process_entries() {
    let source = SourceFile::new(
        FileId(279),
        "library-errors.rinzu",
        format!("{BASE}fn main(){{fail();}}"),
    );
    compile_source_with_options(
        &source,
        &CompilerOptions {
            is_library: true,
            ..Default::default()
        },
    )
    .unwrap();
}
#[test]
fn backend_fixture_catches_the_complete_failure_list_before_returning() {
    compile_source(&SourceFile::new(
        FileId(279),
        "main-error-fixture.rinzu",
        include_str!("../../codegen/tests/fixtures/error-main-boundaries.rinzu"),
    ))
    .unwrap();
}
