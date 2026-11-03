use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Failed{}
async fn fail()->Int{throw Failed(message:"failure");}
async fn safe()->Int{return 7;}
async fn observe(value:Future<Int>){try{let received=await value;}catch errors{}}
"#;
fn accepted(text: &str) {
    compile_source(&SourceFile::new(
        FileId(272),
        "future-capture.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap();
}
fn rejected(text: &str, code: DiagnosticCode) {
    let errors = compile_source(&SourceFile::new(
        FileId(272),
        "future-capture.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap_err();
    assert!(
        errors.iter().any(|error| error.code == code
            && (code != DiagnosticCode::TypeMismatch
                || error.message.contains("retain Future")
                || error.message.contains("native boundary"))),
        "{errors:#?}"
    );
}
#[test]
fn an_async_closure_can_observe_a_captured_future() {
    accepted(
        "fn main(){let pending=fail();let receiver=async fn(){try{let received=await pending;}catch errors{}};receiver();}",
    );
    rejected(
        "fn main(){let pending=fail();let receiver=async fn(){try{let received=await pending;}catch errors{}};}",
        DiagnosticCode::FutureDiscarded,
    );
}
#[test]
fn a_returned_closure_preserves_the_future_until_invocation() {
    accepted(
        "fn make()->fn()->Future<Int>{let pending=fail();return fn()->Future<Int>{return pending;};}async fn main(){let factory=make();try{let received=await factory();}catch errors{}}",
    );
    rejected(
        "fn make()->fn()->Future<Int>{let pending=fail();return fn()->Future<Int>{return pending;};}fn main(){let factory=make();}",
        DiagnosticCode::FutureDiscarded,
    );
}
#[test]
fn calling_a_getter_without_observing_its_result_is_still_a_discard() {
    rejected(
        "fn main(){let pending=fail();let get=fn()->Future<Int>{return pending;};get();}",
        DiagnosticCode::FutureDiscarded,
    );
    accepted(
        "fn main(){let pending=fail();let get=fn()->Future<Int>{return pending;};let receiver=observe(get());}",
    );
}
#[test]
fn capturing_an_alias_does_not_duplicate_the_observation_responsibility() {
    accepted(
        "async fn main(){let pending=fail();let unused=fn()->Future<Int>{return pending;};try{let received=await pending;}catch errors{}}",
    );
    rejected(
        "async fn main(){let pending=fail();let unused=async fn(){try{let received=await pending;}catch errors{}};let invalid=CInt8(300);unused();}",
        DiagnosticCode::FutureDiscarded,
    );
}
#[test]
fn generic_observers_and_record_fields_can_invoke_captured_receivers() {
    accepted(
        "fn run<T>(receiver:fn()->T)->T{return receiver();}fn main(){let pending=fail();let holder={receive=async fn(){try{let received=await pending;}catch errors{}}};let receipt=run<Future<Unit>>(holder.receive);}",
    );
}
#[test]
fn error_payloads_reject_even_successful_or_already_observed_captured_futures() {
    for task in ["safe()", "fail()"] {
        rejected(
            &format!(
                "error Stored{{payload:fn()->Future<Int>}}async fn main(){{let pending={task};try{{let received=await pending;}}catch errors{{}}let captured=fn()->Future<Int>{{return pending;}};let error=Stored(message:\"failure\",payload:captured);}}"
            ),
            DiagnosticCode::TypeMismatch,
        );
    }
}
#[test]
fn a_capture_free_payload_does_not_inherit_other_variants_captures() {
    accepted(
        "error Stored{payload:fn()->Future<Int>}fn main(){let pending=safe();let other=fn()->Future<Int>{return pending;};let error=Stored(message:\"failure\",payload:async fn()->Int{return 7;});}",
    );
}
#[test]
fn forwarding_a_function_to_a_payload_keeps_its_retention_constraint() {
    rejected(
        "error Stored{payload:fn()->Future<Int>}fn wrap(value:fn()->Future<Int>)->Stored{return Stored(message:\"failure\",payload:value);}fn main(){let pending=safe();let error=wrap(fn()->Future<Int>{return pending;});}",
        DiagnosticCode::TypeMismatch,
    );
    accepted(
        "error Stored{payload:fn()->Future<Int>}fn wrap(value:fn()->Future<Int>)->Stored{return Stored(message:\"failure\",payload:value);}fn main(){let error=wrap(async fn()->Int{return 7;});}",
    );
}
#[test]
fn native_callback_private_captures_can_keep_errors_but_not_futures() {
    rejected(
        "native {fn invoke(handler:fn()->CInt32)->CInt32 symbol \"invoke\";}fn main(){let pending=safe();let value=invoke(fn()->CInt32{let copy=pending;return CInt32(0);});}",
        DiagnosticCode::TypeMismatch,
    );
    accepted(
        "native {fn invoke(handler:fn()->CInt32)->CInt32 symbol \"invoke\";}fn main(){let error=Failed(message:\"failure\");let value=invoke(fn()->CInt32{let copy=error;return CInt32(0);});}",
    );
    accepted(
        "native {fn invoke(handler:fn()->CInt32)->CInt32 symbol \"invoke\";}fn main(){let prefix=CInt32(1);let pending=safe();let other=fn()->CInt32{let copy=pending;return prefix;};let value=invoke(fn()->CInt32{return prefix;});}",
    );
}
#[test]
fn mutable_payload_fields_cannot_hide_a_future_capture_after_construction() {
    rejected(
        "class Holder{apply:fn()->Future<Int>}error Stored{payload:Holder}fn main(){let pending=safe();let holder=Holder(apply:async fn()->Int{return 7;});let error=Stored(message:\"failure\",payload:holder);holder.apply=fn()->Future<Int>{return pending;};}",
        DiagnosticCode::TypeMismatch,
    );
}

#[test]
fn recursive_payload_forwarding_has_a_finite_retention_contract() {
    accepted(
        "class Link{const next:Option<Link>,const make:fn()->Future<Int>}error Stored{payload:fn()->Future<Int>}fn wrap(node:Link)->Stored{switch node.next{case Option.None:{return Stored(message:\"failure\",payload:node.make);}case Option.Some(child):{return wrap(child);}}}fn main(){let node=Link(next:Option<Link>.None(),make:async fn()->Int{return 7;});let error=wrap(node);}",
    );
    rejected(
        "class Link{const next:Option<Link>,const make:fn()->Future<Int>}error Stored{payload:fn()->Future<Int>}fn wrap(node:Link)->Stored{switch node.next{case Option.None:{return Stored(message:\"failure\",payload:node.make);}case Option.Some(child):{return wrap(child);}}}fn main(){let pending=safe();let node=Link(next:Option<Link>.None(),make:fn()->Future<Int>{return pending;});let error=wrap(node);}",
        DiagnosticCode::TypeMismatch,
    );
}
