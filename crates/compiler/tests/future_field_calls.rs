use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
import rinzu.list;
error Lost{}
async fn fail(){throw Lost(message:"failure");}
async fn safe(){}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
class Cell{future:Future<Unit>}
fn save(cell:Cell){cell.future=fail();}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(266),
        "future-field-calls.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let errors = compile_source(&SourceFile::new(
        FileId(266),
        "future-field-calls.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}
#[test]
fn a_sync_callee_can_save_a_task_for_its_caller_to_observe() {
    accepted(
        "fn main(){let cell=Cell(future:safe());save(cell);let receiver=observe(cell.future);}",
    );
    lost("fn main(){let cell=Cell(future:safe());save(cell);}");
}
#[test]
fn a_callers_prior_field_remains_its_responsibility_after_overwrite() {
    lost("fn main(){let cell=Cell(future:fail());save(cell);let receiver=observe(cell.future);}");
    accepted(
        "fn main(){let cell=Cell(future:fail());let original=cell.future;save(cell);let before=observe(original);let after=observe(cell.future);}",
    );
}
#[test]
fn aliases_in_different_parameters_follow_the_actual_write_order() {
    let helper = "fn change(first:Cell,second:Cell){first.future=fail();second.future=safe();}";
    lost(&format!(
        "{helper}fn main(){{let cell=Cell(future:safe());change(cell,cell);let receiver=observe(cell.future);}}"
    ));
    accepted(&format!(
        "{helper}fn main(){{let first=Cell(future:safe());let second=Cell(future:safe());change(first,second);let receiver=observe(first.future);}}"
    ));
}
#[test]
fn saving_and_reading_through_aliases_in_one_callee_can_observe_the_new_task() {
    accepted(
        "fn saveAndObserve(first:Cell,second:Cell){first.future=fail();let receiver=observe(second.future);}fn main(){let cell=Cell(future:safe());saveAndObserve(cell,cell);}",
    );
    lost(
        "fn saveAndObserve(first:Cell,second:Cell){first.future=fail();let receiver=observe(second.future);}fn main(){let first=Cell(future:safe());let second=Cell(future:safe());saveAndObserve(first,second);}",
    );
}
#[test]
fn separate_calls_and_repeated_calls_have_distinct_task_identities() {
    accepted(
        "fn main(){let first=Cell(future:safe());let second=Cell(future:safe());save(first);save(second);let one=observe(first.future);let two=observe(second.future);}",
    );
    lost(
        "fn main(){let cell=Cell(future:safe());save(cell);save(cell);let receiver=observe(cell.future);}",
    );
}
#[test]
fn an_error_after_a_store_still_transfers_the_pending_task_to_the_caller() {
    let helper = "fn saveThenFail(cell:Cell){save(cell);let invalid=CInt8(300);}";
    accepted(&format!(
        "{helper}fn main(){{let cell=Cell(future:safe());try{{saveThenFail(cell);}}catch errors{{}}let receiver=observe(cell.future);}}"
    ));
    lost(&format!(
        "{helper}fn main(){{let cell=Cell(future:safe());try{{saveThenFail(cell);}}catch errors{{}}}}"
    ));
}

#[test]
fn caller_locals_survive_callee_names_and_nested_calls() {
    accepted(
        "fn nested(value:Cell){let pending=fail();value.future=pending;}fn main(){let pending=fail();let value=Cell(future:safe());nested(value);let original=observe(pending);let stored=observe(value.future);}",
    );
}

#[test]
fn appending_through_a_mutable_field_preserves_the_complete_task_family() {
    let declarations = "class Batch{values:List<Future<Unit>>}fn append(batch:Batch){let pending=fail();try{batch.values=rinzu.list.append(batch.values,pending);}catch[rinzu.list.LengthOverflow]{let receiver=observe(pending);}}async fn observeList(values:List<Future<Unit>>){var index=0;while index<rinzu.list.length(values){switch rinzu.list.get(values,index){case Option.Some(value):{let receiver=observe(value);}case Option.None:{}}index=index+1;}}";
    accepted(&format!(
        "{declarations}fn main(){{let batch=Batch(values:[]);var index=0;while index<3{{append(batch);index=index+1;}}let receiver=observeList(batch.values);}}"
    ));
    lost(&format!(
        "{declarations}fn main(){{let batch=Batch(values:[]);var index=0;while index<3{{append(batch);batch.values=[];index=index+1;}}let receiver=observeList(batch.values);}}"
    ));
}

#[test]
fn unknown_callbacks_invalidate_fields_previously_written_in_an_async_body() {
    lost(
        "native{fn invoke(handler:fn(Int)->Int)->Int symbol \"invoke\";}class Flag{enabled:Bool}async fn consume(flag:Flag,pending:Future<Unit>){flag.enabled=true;let result=invoke(fn(value:Int)->Int fallback 0{flag.enabled=false;return value;});if flag.enabled{try{await pending;}catch errors{}}}fn main(){let pending=fail();let flag=Flag(enabled:false);let receiver=consume(flag,pending);}",
    );
}
