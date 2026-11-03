use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Lost{}
async fn fail(){throw Lost(message:"failure");}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
async fn observeList(values:List<Future<Unit>>){var index=0;while index<listLength(values){switch listGet(values,index){case Option.Some(value):{let observer=observe(value);}case Option.None:{}}index=index+1;}}
"#;
const BUILDER: &str = r#"
fn build(count:Int)->List<Future<Unit>> {
    var output:List<Future<Unit>> = [];
    var index=0;
    while index<count {output=listAppend(output,fail());index=index+1;}
    return output;
}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(260),
        "future-collections.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(260),
        "future-collections.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::FutureDiscarded),
        "{diagnostics:#?}"
    );
}
#[test]
fn a_growing_list_transfers_every_pending_task_to_its_receiver() {
    accepted(&format!(
        "{BUILDER} fn main(){{let values=build(5);let observer=observeList(values);}}"
    ));
    lost(&format!("{BUILDER} fn main(){{let abandoned=build(5);}}"));
}

#[test]
fn dropping_the_collection_or_retaining_one_member_does_not_cover_the_family() {
    lost(&format!(
        "{} fn main(){{let values=build(5);let receiver=observeList(values);}}",
        BUILDER.replace("return output;", "return [];")
    ));
    lost(
        "fn build(count:Int)->List<Future<Unit>>{var last=fail();var index=0;while index<count{last=fail();index=index+1;}return [last];}fn main(){let receiver=observeList(build(5));}",
    );
}

#[test]
fn conditionally_saving_only_some_generations_does_not_transfer_all_tasks() {
    let partial = BUILDER.replace(
        "output=listAppend(output,fail());",
        "let pending=fail();if index==0{output=listAppend(output,pending);}",
    );
    lost(&format!(
        "{partial}fn main(){{let receiver=observeList(build(5));}}"
    ));
}

#[test]
fn a_normal_error_before_append_retains_the_created_tasks_obligation() {
    let interrupted = BUILDER.replace(
        "output=listAppend(output,fail());",
        "let pending=fail();let bad=CInt8(300);output=listAppend(output,pending);",
    );
    lost(&format!(
        "{interrupted}async fn main(){{try{{let receiver=observeList(build(5));}}catch errors{{}}}}"
    ));
}

#[test]
fn an_alias_of_the_complete_collection_can_receive_it_after_rebinding() {
    let aliased = BUILDER.replace("return output;", "let saved=output;output=[];return saved;");
    accepted(&format!(
        "{aliased}fn main(){{let receiver=observeList(build(5));}}"
    ));
}

#[test]
fn observing_one_factory_result_does_not_observe_another_call() {
    lost(&format!(
        "{BUILDER}fn main(){{let first=build(5);let second=build(5);let receiver=observeList(first);}}"
    ));
    accepted(&format!(
        "{BUILDER}fn main(){{let first=build(5);let second=build(5);let firstReceiver=observeList(first);let secondReceiver=observeList(second);}}"
    ));
}

#[test]
fn replacing_an_observed_batch_does_not_reuse_its_receipt_for_new_tasks() {
    accepted(&format!(
        "{BUILDER}fn main(){{var previous:List<Future<Unit>> = [];var index=0;while index<3{{let current=build(2);let receiver=observeList(previous);previous=current;index=index+1;}}let finalReceiver=observeList(previous);}}"
    ));
    lost(&format!(
        "{BUILDER}fn main(){{var old:List<Future<Unit>> = [];var index=0;while index<3{{let current=build(2);if index==0{{let initial=observeList(current);old=current;}}else{{let stale=observeList(old);}}index=index+1;}}}}"
    ));
}

#[test]
fn removal_and_replacement_cannot_discharge_the_original_collection() {
    for operation in ["listRemove(values,0)", "listSet(values,0,fail())"] {
        lost(&format!(
            "{BUILDER}fn main(){{let values=build(3);switch {operation}{{case Option.Some(changed):{{let receiver=observeList(changed);}}case Option.None:{{let receiver=observeList(values);}}}}}}"
        ));
        let safe_operation = operation.replace("fail()", "replacement");
        accepted(&format!(
            "{BUILDER}fn main(){{let values=build(3);let original=observeList(values);let replacement=fail();let replacementReceiver=observe(replacement);switch {safe_operation}{{case Option.Some(changed):{{let receiver=observeList(changed);}}case Option.None:{{}}}}}}"
        ));
    }
}

#[test]
fn an_observed_collection_does_not_reintroduce_obligations_when_returned() {
    accepted(&format!(
        "{BUILDER}fn handled()->List<Future<Unit>>{{let values=build(3);let observer=observeList(values);return values;}}fn main(){{let values=handled();}}"
    ));
}

#[test]
fn an_immutable_field_preserves_the_complete_family_across_a_loop_join() {
    let builder = "class Batch{const values:List<Future<Unit>>}fn build(count:Int)->Batch{var output=Batch(values:[]);var index=0;while index<count{output=Batch(values:listAppend(output.values,fail()));index=index+1;}return output;}";
    accepted(&format!(
        "{builder}fn main(){{let batch=build(3);let observer=observeList(batch.values);}}"
    ));
    lost(&format!("{builder}fn main(){{let abandoned=build(3);}}"));
}
