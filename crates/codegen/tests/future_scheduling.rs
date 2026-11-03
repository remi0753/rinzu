use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile, compile_source};

fn check_output(text: &str, stress: bool, expected: &[u8]) {
    let artifact = compile_source(&SourceFile::new(FileId(387), "queued.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: stress,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:#?}"
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert_eq!(run.stdout, expected);
}

fn check(text: &str, stress: bool) {
    check_output(text, stress, b"");
}

#[test]
fn observed_ledger_retirement_preserves_returned_future_aliases_across_gc() {
    let text = include_str!("fixtures/observed-ledger-alias.rinzu");
    check_output(text, false, b"returned-future-ok\n");
    check_output(text, true, b"returned-future-ok\n");
}

#[test]
fn future_aliases_survive_body_finally_and_repeated_await() {
    let text = include_str!("fixtures/future-scope-alias.rinzu");
    check_output(text, false, b"scope-alias:512\n");
    check_output(text, true, b"scope-alias:512\n");
}

#[test]
fn a_nofail_observer_records_failure_while_its_parent_keeps_running() {
    let text = include_str!("fixtures/future-app-observer.rinzu");
    check_output(text, false, b"retained payload\n");
    check_output(text, true, b"retained payload\n");
}

#[test]
fn discarded_task_traps_remain_control_failures() {
    let artifact = compile_source(&SourceFile::new(
        FileId(395),
        "future-control.rinzu",
        "async fn bad() { let invalid = 1/0; } async fn main() { bad(); }",
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.resources_returned, "{run:#?}");
    assert_eq!(
        run.failure_details.unwrap().primary.cause,
        rinzu_runtime::failure::FailureCause::ArithmeticTrap
    );
    assert!(run.caught_errors.is_empty());
}

#[test]
fn startup_validation_throws_at_the_call_and_does_not_register_a_future() {
    let artifact = compile_source(&SourceFile::new(
        FileId(394),
        "future-start-errors.rinzu",
        include_str!("fixtures/future-start-errors.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:#?}"
    );
    assert_eq!(run.stdout, b"timer:0\npath:0\nargument:0\nbody:1\n1\n2\n");
    assert_eq!(run.future_admissions.len(), 1);
    assert_eq!(run.caught_errors.len(), 6);
}

#[test]
fn discarded_results_and_future_cycles_are_reclaimed_during_repeated_batches() {
    let artifact = compile_source(&SourceFile::new(
        FileId(392),
        "future-gc.rinzu",
        include_str!("fixtures/future-gc.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:#?}"
    );
    assert_eq!(
        run.stdout,
        b"4096\n64\nretained through finally\nretained for queued observer\n"
    );
    assert!(
        run.peak_future_records < 256,
        "peak records: {}",
        run.peak_future_records
    );
    assert!(
        run.heap.peak_objects < 256,
        "peak objects: {}",
        run.heap.peak_objects
    );
    assert!(
        run.gc_retired_futures >= 4096,
        "retired: {}",
        run.gc_retired_futures
    );
    assert_eq!(run.remaining_future_records, 0);
}

#[test]
fn future_metadata_triggers_collection_even_without_managed_allocations() {
    let artifact = compile_source(&SourceFile::new(
        FileId(393),
        "future-gc-unit.rinzu",
        include_str!("fixtures/future-gc-unit.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:#?}"
    );
    assert_eq!(run.heap.peak_objects, 0);
    assert!(
        run.peak_future_records <= 1152,
        "peak records: {}",
        run.peak_future_records
    );
    assert!(run.gc_retired_futures >= 4096);
    assert_eq!(run.remaining_future_records, 0);
}

#[test]
fn tasks_outlive_successful_and_failed_parents_and_continue_after_main() {
    check_output(
        include_str!("fixtures/future-lifetime.rinzu"),
        true,
        b"17\nparent only\nmain complete\nlate:1\nlate:2\n",
    );
}

#[test]
fn synchronous_main_can_start_work_that_the_runtime_retains_and_drains() {
    check_output(
        include_str!("fixtures/future-sync-lifetime.rinzu"),
        true,
        b"sync main complete\nretained after sync main\n",
    );
}

#[test]
fn calls_return_before_body_execution_and_nested_factories_preserve_rooted_arguments() {
    let text = r#"
class State {value:Int,text:String}
async fn child(state:State)->String {state.value=state.value+1;return state.text;}
fn launch(state:State)->Future<String> {return child(state);}
async fn main() {
    let state=State(value:0,text:"界\u{0}🙂");
    let pending=launch(state);
    if state.value!=0 || futureReady(pending) {panic("inlineBody");}
    var i=0;while i<100 {let moved=["queued","arguments"];i=i+1;}
    let result=await pending;
    if result!=state.text || state.value!=1 || !futureReady(pending) {panic("missingBody");}
    let again=await pending;
    if again!=result {panic("changedResult");}
}
"#;
    for stress in [false, true] {
        check(text, stress);
    }
}

#[test]
fn accepted_tasks_are_not_limited_to_4096_and_do_not_require_an_await_to_start() {
    check(
        r#"
class State {value:Int}
async fn child(state:State) {state.value=state.value+1;}
async fn inspect(state:State)->Int {return state.value;}
async fn main() {
    let state=State(value:0);
    var i=0;while i<8192 {child(state);i=i+1;}
    if state.value!=0 {panic("inlineBody");}
    let total=await inspect(state);
    if total!=8192 {panic("lostTask");}
}

"#,
        false,
    );
}

#[test]
fn cancelling_a_queued_task_does_not_execute_its_body_or_unentered_finally() {
    let text = r#"
import rinzu.stdio;
class State {value:Int}
async fn child(state:State) {
    try {state.value=1;} finally {state.value=2;}
}
async fn main() {
    let state=State(value:0);
    let pending=child(state);
    let cancelled=cancelFuture(pending);
    if !cancelled {panic("lostCancellation");}
    try {await pending;}
    finally {try {rinzu.stdio.writeLine(state.value);} catch errors {}}
}
"#;
    let artifact =
        compile_source(&SourceFile::new(FileId(388), "cancel-queued.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        run.failure_details.as_ref().unwrap().primary.cause,
        rinzu_runtime::failure::FailureCause::Cancelled
    );
    assert_eq!(run.stdout, b"0\n");
    assert!(run.resources_returned, "{run:#?}");
}
