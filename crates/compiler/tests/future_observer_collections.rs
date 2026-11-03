use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Failed{}
async fn fail(){throw Failed(message:"failure");}
async fn safe(){}
async fn receive(value:Future<Unit>){try{await value;}catch errors{}}
async fn waitAll(jobs:List<Future<Unit>>){var index=0;while index<listLength(jobs){switch listGet(jobs,index){case Option.Some(job):{await job;}case Option.None:{}}index=index+1;}}
async fn settle(jobs:List<Future<Unit>>){var index=0;while index<listLength(jobs){switch listGet(jobs,index){case Option.Some(job):{try{await job;}catch errors{}}case Option.None:{}}index=index+1;}}
fn make(count:Int)->List<Future<Unit>>{var jobs:List<Future<Unit>> = [];var index=0;while index<count{let pending=fail();let job=receive(pending);jobs=listAppend(jobs,job);index=index+1;}return jobs;}
"#;
fn accepted(text: &str) {
    match compile_source(&SourceFile::new(
        FileId(274),
        "observer-collections.rinzu",
        format!("{BASE}{text}"),
    )) {
        Ok(_) => {}
        Err(errors) => panic!("{errors:#?}"),
    }
}
fn lost(text: &str) {
    let result = compile_source(&SourceFile::new(
        FileId(274),
        "observer-collections.rinzu",
        format!("{BASE}{text}"),
    ));
    let errors = match result {
        Ok(_) => panic!("a partial observation was accepted"),
        Err(errors) => errors,
    };
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}
#[test]
fn cancelling_a_generated_receiver_keeps_its_private_failure_responsibility() {
    lost(
        "async fn main(){let jobs=make(3);switch listGet(jobs,0){case Option.Some(first):{let cancelled=cancelFuture(first);}case Option.None:{}}}",
    );
}
#[test]
fn completing_every_receiver_allows_scope_shutdown() {
    accepted(
        "async fn main(){let jobs=make(3);let all=waitAll(jobs);await all;let shutdown=requestShutdown();}",
    );
}
#[test]
fn a_sync_factory_preserves_complete_receiver_observation() {
    accepted(
        "fn start(jobs:List<Future<Unit>>)->Future<Unit>{return waitAll(jobs);}async fn main(){let jobs=make(3);let all=start(jobs);await all;let shutdown=requestShutdown();}",
    );
}
#[test]
fn completing_one_receiver_does_not_complete_the_generated_family() {
    lost(
        "async fn main(){let jobs=make(3);switch listGet(jobs,0){case Option.Some(one):{await one;}case Option.None:{}}let shutdown=requestShutdown();}",
    );
}
#[test]
fn observing_a_partial_receiver_collection_is_not_a_full_completion() {
    lost(
        "fn partial(count:Int)->List<Future<Unit>>{var jobs:List<Future<Unit>> = [];var index=0;while index<count{let pending=fail();let job=receive(pending);if index==0{jobs=listAppend(jobs,job);}index=index+1;}return jobs;}async fn main(){let jobs=partial(3);let all=waitAll(jobs);await all;let shutdown=requestShutdown();}",
    );
}
#[test]
fn partial_value_receivers_cannot_hide_invalidated_field_receivers() {
    lost(
        "class Cell{future:Future<Unit>}async fn fromCell(cell:Cell){try{await cell.future;}catch errors{}}fn overwrite(cells:List<Cell>){var index=0;while index<listLength(cells){switch listGet(cells,index){case Option.Some(cell):{cell.future=safe();}case Option.None:{}}index=index+1;}}async fn main(){var cells:List<Cell> = [];var index=0;while index<3{let pending=fail();let cell=Cell(future:pending);let field=fromCell(cell);if index==0{let value=receive(pending);}cells=listAppend(cells,cell);index=index+1;}overwrite(cells);}",
    );
}
#[test]
fn completing_a_dispatcher_does_not_complete_its_per_element_children() {
    lost(
        "async fn dispatch(jobs:List<Future<Unit>>){var index=0;while index<listLength(jobs){switch listGet(jobs,index){case Option.Some(job):{let child=receive(job);}case Option.None:{}}index=index+1;}}async fn main(){let pending=fail();await dispatch([pending]);let shutdown=requestShutdown();}",
    );
}
#[test]
fn completing_awaited_per_element_children_allows_shutdown() {
    accepted(
        "async fn dispatch(jobs:List<Future<Unit>>){var index=0;while index<listLength(jobs){switch listGet(jobs,index){case Option.Some(job):{await receive(job);}case Option.None:{}}index=index+1;}}async fn main(){let pending=fail();await dispatch([pending]);let shutdown=requestShutdown();}",
    );
}
#[test]
fn early_failure_cannot_observe_later_elements() {
    lost(
        "async fn main(){let first=fail();let second=fail();try{await waitAll([first,second]);}catch errors{}}",
    );
}
#[test]
fn restored_responsibilities_are_not_covered_by_a_partial_old_collection() {
    lost(
        "async fn main(){var saved:List<Future<Unit>> = [];var receivers:List<Future<Unit>> = [];var index=0;while index<3{let pending=fail();let job=receive(pending);receivers=listAppend(receivers,job);if index==0{saved=listAppend(saved,pending);}index=index+1;}let shutdown=requestShutdown();await settle(saved);}",
    );
}
#[test]
fn restored_responsibilities_can_be_observed_through_a_complete_collection() {
    accepted(
        "async fn main(){var saved:List<Future<Unit>> = [];var index=0;while index<3{let pending=fail();let job=receive(pending);saved=listAppend(saved,pending);index=index+1;}let shutdown=requestShutdown();await settle(saved);}",
    );
}
#[test]
fn independent_completed_batches_keep_separate_receipts() {
    accepted(
        "async fn main(){var index=0;while index<3{let jobs=make(3);await waitAll(jobs);index=index+1;}let shutdown=requestShutdown();}",
    );
    lost(
        "async fn main(){let first=make(3);let second=make(3);await waitAll(first);let shutdown=requestShutdown();}",
    );
}
#[test]
fn another_complete_receiver_survives_one_receiver_cancellation() {
    accepted(
        "async fn main(){let pending=fail();let first=receive(pending);let second=receive(pending);let cancelled=cancelFuture(first);await waitAll([second]);let shutdown=requestShutdown();}",
    );
}
#[test]
fn map_key_snapshots_prove_completion_of_their_own_receivers() {
    let helper = "async fn waitMap(jobs:Map<String,Future<Unit>>){let keys=mapKeys(jobs);var index=0;while index<listLength(keys){switch listGet(keys,index){case Option.Some(key):{switch mapGet(jobs,key){case Option.Some(job):{await job;}case Option.None:{}}}case Option.None:{}}index=index+1;}}";
    accepted(&format!(
        "{helper}async fn main(){{let pending=fail();let jobs={{\"first\":receive(pending)}};await waitMap(jobs);let shutdown=requestShutdown();}}"
    ));
    lost(&format!(
        "{helper}async fn main(){{let first=fail();let second=fail();let jobs={{\"first\":receive(first),\"second\":receive(second)}};await waitMap(mapRemove(jobs,\"second\"));let shutdown=requestShutdown();}}"
    ));
}
#[test]
fn a_waiter_registered_before_restoration_cannot_complete_unsaved_producers() {
    lost(
        "async fn main(){var saved:List<Future<Unit>> = [];var index=0;while index<3{let pending=fail();let job=receive(pending);if index==0{saved=listAppend(saved,pending);}index=index+1;}let waiter=settle(saved);let shutdown=requestShutdown();await waiter;}",
    );
}
#[test]
fn restored_older_members_invalidate_pending_only_coverage() {
    let setup = "var saved:List<Future<Unit>> = [];var index=0;while index<3{let pending=fail();if index<2{let job=receive(pending);}else{saved=listAppend(saved,pending);}index=index+1;}";
    lost(&format!(
        "async fn main(){{{setup}let shutdown=requestShutdown();await settle(saved);}}"
    ));
    lost(&format!(
        "fn stop()->Int{{let shutdown=requestShutdown();return 0;}}async fn both(jobs:List<Future<Unit>>,ignored:Int){{await settle(jobs);}}async fn main(){{{setup}await both(saved,stop());}}"
    ));
}
