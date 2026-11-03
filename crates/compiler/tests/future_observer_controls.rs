use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Failed{}
async fn fail(){throw Failed(message:"failure");}
async fn receive(value:Future<Unit>){try{await value;}catch errors{}}
"#;
fn accepted(text: &str) {
    compile_source(&SourceFile::new(
        FileId(273),
        "observer-controls.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap();
}
fn lost(text: &str) {
    let result = compile_source(&SourceFile::new(
        FileId(273),
        "observer-controls.rinzu",
        format!("{BASE}{text}"),
    ));
    let errors = match result {
        Ok(_) => panic!("a lost observer was accepted"),
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
fn cancel_before_observation_does_not_erase_the_producers_responsibility() {
    lost(
        "async fn main(){let pending=fail();let observer=receive(pending);let cancelled=cancelFuture(observer);}",
    );
    accepted(
        "async fn main(){let pending=fail();let observer=receive(pending);let cancelled=cancelFuture(observer);try{await pending;}catch errors{}}",
    );
}
#[test]
fn an_uncancelled_receiver_can_complete_the_observation() {
    accepted(
        "async fn main(){let pending=fail();let first=receive(pending);let second=receive(pending);let cancelled=cancelFuture(first);await second;}",
    );
}
#[test]
fn cancellation_after_completion_does_not_restore_an_observed_failure() {
    accepted(
        "async fn main(){let pending=fail();let observer=receive(pending);await observer;let cancelled=cancelFuture(observer);}",
    );
}
#[test]
fn a_returned_observer_retains_private_failure_responsibility() {
    lost(
        "fn make()->Future<Unit>{return receive(fail());}async fn main(){let observer=make();let cancelled=cancelFuture(observer);}",
    );
    accepted(
        "fn make()->Future<Unit>{return receive(fail());}async fn main(){let observer=make();await observer;let cancelled=cancelFuture(observer);}",
    );
}

#[test]
fn cancellation_cannot_restore_a_stale_field_observation() {
    lost(
        "class Cell{future:Future<Unit>}async fn fromCell(cell:Cell){try{await cell.future;}catch errors{}}async fn safe(){}async fn main(){let pending=fail();let cell=Cell(future:pending);let field=fromCell(cell);let value=receive(pending);cell.future=safe();let cancelled=cancelFuture(value);await field;}",
    );
}

#[test]
fn waiting_for_a_parent_does_not_complete_its_unawaited_receiver() {
    lost(
        "type Started={child:Future<Unit>};async fn start(value:Future<Unit>)->Started{let child=receive(value);return {child=child};}async fn main(){let pending=fail();let parent=start(pending);let result=await parent;let cancelled=cancelFuture(result.child);}",
    );
    accepted(
        "type Started={child:Future<Unit>};async fn start(value:Future<Unit>)->Started{let child=receive(value);return {child=child};}async fn main(){let pending=fail();let parent=start(pending);let result=await parent;await result.child;let cancelled=cancelFuture(parent);}",
    );
}

#[test]
fn control_helpers_preserve_the_cancelled_receiver_identity() {
    lost(
        "fn stop(observer:Future<Unit>){let cancelled=cancelFuture(observer);}async fn main(){let pending=fail();let observer=receive(pending);stop(observer);}",
    );
    accepted(
        "fn stop(observer:Future<Unit>){let cancelled=cancelFuture(observer);}async fn main(){let pending=fail();let first=receive(pending);let second=receive(pending);stop(first);await second;}",
    );
}

#[test]
fn repeated_parent_await_cannot_reintroduce_a_cancelled_childs_observation() {
    lost(
        "type Started={child:Future<Unit>};async fn start(value:Future<Unit>)->Started{let child=receive(value);return {child=child};}async fn main(){let pending=fail();let parent=start(pending);let result=await parent;let cancelled=cancelFuture(result.child);let again=await parent;}",
    );
    accepted(
        "type Started={child:Future<Unit>};async fn start(value:Future<Unit>)->Started{let child=receive(value);return {child=child};}async fn main(){let pending=fail();let parent=start(pending);let result=await parent;let cancelled=cancelFuture(parent);}",
    );
}
#[test]
fn a_cancelled_returned_observer_does_not_forge_a_successful_transfer() {
    lost(
        "async fn maybe(flag:Bool){if flag{throw Failed(message:\"failure\");}}fn make(flag:Bool)->Future<Unit>{let pending=maybe(flag);let observer=receive(pending);let cancelled=cancelFuture(observer);return observer;}async fn main(){let observer=make(true);await observer;}",
    );
}
#[test]
fn a_completed_field_reader_can_leave_a_child_receiving_the_snapshot() {
    accepted(
        "class Cell{future:Future<Unit>}async fn fromCell(cell:Cell){let child=receive(cell.future);}async fn safe(){}async fn main(){let pending=fail();let cell=Cell(future:pending);let parent=fromCell(cell);await parent;cell.future=safe();}",
    );
}
#[test]
fn recursive_observer_receipts_reach_a_finite_summary() {
    accepted(
        "async fn safe(){}async fn recurse(gate:Future<Unit>){let pending=fail();let observer=receive(pending);if futureReady(gate){let child=recurse(gate);await child;}}async fn main(){let gate=safe();let observer=recurse(gate);}",
    );
    accepted(
        "async fn recurse(value:Future<Unit>){if futureReady(value){let child=recurse(value);await child;}else{try{await value;}catch errors{}}}async fn main(){let pending=fail();let observer=recurse(pending);}",
    );
    accepted(
        "async fn recurse(value:Future<Unit>,again:Bool){if again{let child=recurse(value,again);await child;}else{try{await value;}catch errors{}}}fn main(){let pending=fail();let observer=recurse(pending,false);}",
    );
}
#[test]
fn a_native_callback_cannot_hide_scope_shutdown_from_the_observation_proof() {
    lost(
        "native {fn invoke(handler:fn()->CInt32)->CInt32 symbol \"invoke\";}async fn main(){let pending=fail();let observer=receive(pending);try{let result=invoke(fn()->CInt32{let shutdown=requestShutdown();return CInt32(0);});}catch errors{}}",
    );
    accepted(
        "native {fn invoke(handler:fn()->CInt32)->CInt32 symbol \"invoke\";}async fn main(){let zero=CInt32(0);let unused=fn()->CInt32{let shutdown=requestShutdown();return zero;};let pending=fail();let observer=receive(pending);try{let result=invoke(fn()->CInt32{return zero;});}catch errors{}}",
    );
}
