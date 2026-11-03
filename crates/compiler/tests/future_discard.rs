use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(271), "future-discard.rinzu", text))
}

fn accepted(text: &str) {
    compile(text).unwrap();
}

fn lost(text: &str) {
    let errors = compile(text).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}

const FAIL: &str = "error Failed{} async fn fail(){throw Failed(message:\"failure\");}";

#[test]
fn infallible_future_values_can_be_discarded_in_sync_and_async_code() {
    accepted(
        "async fn safe()->Int{return 7;}fn helper(){safe();}async fn main(){safe();helper();}",
    );
    accepted("async fn safe(){}fn main(){safe();}");
    accepted(&format!(
        "{FAIL} async fn safe(){{try{{await fail();}}catch errors{{}}}}fn main(){{safe();}}"
    ));
}

#[test]
fn try_all_does_not_adopt_a_discarded_failing_task() {
    for body in [
        "fail();",
        "try{fail();}catch errors{}",
        "try all{fail();}catch errors{}",
    ] {
        lost(&format!("{FAIL}fn main(){{{body}}}"));
    }
    accepted("async fn safe(){}fn main(){try all{safe();}catch errors{}}");
}

#[test]
fn discarding_one_alias_does_not_discard_the_task_if_another_observes_it() {
    accepted(&format!(
        "{FAIL}async fn main(){{let pending=fail();pending;try{{await pending;}}catch errors{{}}}}"
    ));
    lost(&format!("{FAIL}fn main(){{let pending=fail();pending;}}"));
}

#[test]
fn a_discarded_success_value_cannot_hide_an_inner_failing_future() {
    lost(&format!(
        "{FAIL} type Boxed={{future:Future<Unit>}};async fn make()->Boxed{{return {{future=fail()}};}}fn main(){{make();}}"
    ));
    accepted(
        "async fn safe(){}type Boxed={future:Future<Unit>};async fn make()->Boxed{return {future=safe()};}fn main(){make();}",
    );
}

#[test]
fn failure_handling_itself_must_also_be_infallible_before_discard() {
    lost(&format!(
        "{FAIL}async fn unsafeObserver(){{try{{await fail();}}catch errors{{let invalid=CInt8(300);}}}}fn main(){{unsafeObserver();}}"
    ));
}
