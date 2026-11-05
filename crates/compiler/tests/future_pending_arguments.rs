use rinzu_compiler::{FileId, SourceFile, compile_source};
const BASE: &str = r#"
import rinzu.List;
error Lost{} async fn fail(){throw Lost(message:"lost");} async fn ok(){} async fn forever(){while true{}} async fn observe(value:Future<Unit>){try{let ignored=await value;}catch errors{}}"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(257),
        "pending-arguments.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(257),
        "pending-arguments.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == rinzu_compiler::DiagnosticCode::FutureDiscarded),
        "{diagnostics:#?}"
    );
}
#[test]
fn pending_argument_does_not_remove_the_callers_later_statements() {
    let source = r#"error Lost{} async fn fail(){throw Lost(message:"lost");} async fn forever(){while true{}} async fn observe(value:Future<Unit>){let ignored=await value;} async fn main(){let running=forever();let observer=observe(running);let lost=fail();}"#;
    let accepted =
        compile_source(&SourceFile::new(FileId(257), "pending-probe.rinzu", source)).is_ok();
    assert!(
        !accepted,
        "unexpectedly accepted an unobserved task after an async registration"
    );
}

#[test]
fn an_async_registration_still_returns_a_future_when_its_body_cannot_finish() {
    lost(
        "async fn main(){let running=forever();let observer=observe(running);let abandoned=fail();}",
    );
    accepted("async fn main(){let running=forever();let observer=observe(running);let safe=ok();}");
}
#[test]
fn a_task_created_before_a_nonreturning_await_still_needs_a_receiver() {
    lost(
        "async fn run(value:Future<Unit>){let abandoned=fail();await value;} fn main(){let pending=run(forever());}",
    );
    accepted(
        "async fn run(value:Future<Unit>){await value;let neverCreated=fail();} fn main(){let pending=run(forever());}",
    );
}
#[test]
fn references_and_independent_observers_do_not_add_a_completion_dependency() {
    accepted(
        "async fn ignore(value:Future<Unit>){} async fn main(){let running=forever();let other=fail();await ignore(running);try{await other;}catch[Lost]{}}",
    );
    accepted(
        "async fn handoff(value:Future<Unit>){let managed=observe(value);} async fn main(){let running=forever();let other=fail();await handoff(running);try{await other;}catch[Lost]{}}",
    );
}
#[test]
fn conditional_nonreturning_inputs_keep_the_callers_matching_branch() {
    lost(
        "async fn choose(block:Bool){if block{while true{}}} async fn run(block:Bool){let input=choose(block);let observer=observe(input);let abandoned=fail();}async fn main(){await run(true);}",
    );
}
#[test]
fn sync_factories_and_const_fields_keep_nonreturning_outcomes() {
    lost(
        "fn wrap(value:Future<Unit>)->Future<Unit>{return observe(value);} fn main(){let input=forever();let observer=wrap(input);let abandoned=fail();}",
    );
    lost(
        "class Pending{const future:Future<Unit>} async fn receive(value:Pending){try{await value.future;}catch errors{}} fn main(){let observer=receive(Pending(future:forever()));let abandoned=fail();}",
    );
}

const WAIT_LIST: &str = r#"async fn waitList(values:List<Future<Unit>>){var index=0;while index<rinzu.List.length(values){switch rinzu.List.get(values,index){case Option.Some(value):{try{await value;}catch errors{}}case Option.None:{}}index=index+1;}}"#;
#[test]
fn a_blocking_list_observer_cannot_hide_a_failure_behind_a_nonreturning_element() {
    lost(&format!(
        "{WAIT_LIST} fn main(){{let running=forever();let failing=fail();let observer=waitList([running,failing]);}}"
    ));
    accepted(&format!(
        "{WAIT_LIST} fn main(){{let observer=waitList([ok(),fail()]);}}"
    ));
}
#[test]
fn independent_per_element_observers_keep_receiving_other_tasks() {
    let dispatch = WAIT_LIST.replace(
        "try{await value;}catch errors{}",
        "let observer=observe(value);",
    );
    accepted(&format!(
        "{dispatch} fn main(){{let running=forever();let failing=fail();let observer=waitList([running,failing]);}}"
    ));
}
