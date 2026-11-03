use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Lost{}
async fn fail(){throw Lost(message:"failure");}
async fn safe(){}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
class Cell{future:Future<Unit>}
async fn receive(cell:Cell){try{await cell.future;}catch errors{}}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(268),
        "future-field-observers.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let errors = compile_source(&SourceFile::new(
        FileId(268),
        "future-field-observers.rinzu",
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
fn a_readonly_observer_can_receive_a_field_which_is_not_later_changed() {
    accepted("fn main(){let cell=Cell(future:fail());let receiver=receive(cell);}");
    accepted(
        "fn schedule(cell:Cell){let receiver=receive(cell);}fn main(){let cell=Cell(future:fail());schedule(cell);}",
    );
}
#[test]
fn a_later_store_restores_the_original_future_obligation() {
    lost("fn main(){let cell=Cell(future:fail());let receiver=receive(cell);cell.future=safe();}");
    lost(
        "fn change(cell:Cell){cell.future=safe();}fn main(){let cell=Cell(future:fail());let receiver=receive(cell);change(cell);}",
    );
}
#[test]
fn awaiting_the_receiver_before_the_store_finishes_its_observation() {
    accepted(
        "async fn main(){let cell=Cell(future:fail());let receiver=receive(cell);await receiver;cell.future=safe();}",
    );
}
#[test]
fn repeated_await_of_an_invalidated_receiver_cannot_observe_the_old_field() {
    lost(
        "async fn main(){let cell=Cell(future:fail());let receiver=receive(cell);cell.future=safe();await receiver;await receiver;}",
    );
}
#[test]
fn a_saved_future_is_independent_of_changes_to_the_cell() {
    accepted(
        "fn main(){let cell=Cell(future:fail());let saved=cell.future;let receiver=receive(cell);cell.future=safe();let independent=observe(saved);}",
    );
}
#[test]
fn another_unchanged_receiver_can_still_own_the_same_future() {
    accepted(
        "fn main(){let pending=fail();let first=Cell(future:pending);let second=Cell(future:pending);let one=receive(first);let two=receive(second);first.future=safe();}",
    );
    lost(
        "fn main(){let pending=fail();let first=Cell(future:pending);let second=Cell(future:pending);let one=receive(first);let two=receive(second);first.future=safe();second.future=safe();}",
    );
}
#[test]
fn returning_a_watched_cell_keeps_the_callers_later_mutations_visible() {
    let factory = "type Pair={cell:Cell,receiver:Future<Unit>};fn make()->Pair{let cell=Cell(future:fail());let receiver=receive(cell);return {cell=cell,receiver=receiver};}";
    accepted(&format!("{factory}fn main(){{let pair=make();}}"));
    lost(&format!(
        "{factory}fn main(){{let pair=make();pair.cell.future=safe();}}"
    ));
}
#[test]
fn repeated_task_generations_are_not_observed_by_one_old_receiver() {
    accepted(
        "fn main(){var index=0;while index<4{let cell=Cell(future:fail());let receiver=receive(cell);index=index+1;}}",
    );
    lost(
        "fn main(){var index=0;while index<4{let cell=Cell(future:fail());if index==0{let receiver=receive(cell);}index=index+1;}}",
    );
}

#[test]
fn a_conditional_receiver_cannot_replace_a_different_valid_field_observer() {
    lost(
        "async fn maybe(cell:Cell,enabled:Bool){if enabled{try{await cell.future;}catch errors{}}}fn run(enabled:Bool){let pending=fail();let first=Cell(future:pending);let second=Cell(future:pending);let initial=receive(first);let conditional=maybe(second,enabled);first.future=safe();}fn main(){run(false);}",
    );
}

#[test]
fn an_independent_value_receiver_releases_the_existing_field_dependency() {
    accepted(
        "fn main(){let pending=fail();let cell=Cell(future:pending);let fieldReceiver=receive(cell);let independent=observe(pending);cell.future=safe();}",
    );
}

#[test]
fn cancellation_does_not_prove_a_field_observer_ran() {
    lost(
        "async fn main(){let cell=Cell(future:fail());let receiver=receive(cell);let cancelled=cancelFuture(receiver);}",
    );
    lost(
        "fn stop(value:Future<Unit>){let cancelled=cancelFuture(value);}async fn main(){let cell=Cell(future:fail());let receiver=receive(cell);stop(receiver);await receiver;}",
    );
    accepted(
        "async fn main(){let cell=Cell(future:fail());let receiver=receive(cell);await receiver;let cancelled=cancelFuture(receiver);cell.future=safe();}",
    );
}

#[test]
fn a_pending_writer_can_obtain_a_newly_shared_cell() {
    lost(
        "class Holder{cell:Cell}async fn replace(holder:Holder){holder.cell.future=safe();}fn main(){let holder=Holder(cell:Cell(future:safe()));let writer=replace(holder);let pending=fail();let cell=Cell(future:pending);holder.cell=cell;let receiver=receive(cell);}",
    );
    accepted(
        "class Holder{cell:Cell}async fn replace(holder:Holder){holder.cell.future=safe();}fn main(){let holder=Holder(cell:Cell(future:safe()));let writer=replace(holder);let pending=fail();let cell=Cell(future:pending);holder.cell=cell;let independent=observe(pending);}",
    );
}

#[test]
fn a_returned_receiver_cannot_reintroduce_an_invalidated_observation() {
    lost(
        "fn make()->Future<Unit>{let cell=Cell(future:fail());let receiver=receive(cell);cell.future=safe();return receiver;}async fn main(){let receiver=make();await receiver;}",
    );
    lost(
        "fn redirect(pending:Future<Unit>)->Future<Unit>{let cell=Cell(future:pending);let receiver=receive(cell);cell.future=safe();return receiver;}async fn main(){let pending=fail();let receiver=redirect(pending);await receiver;}",
    );
    accepted(
        "fn make()->Future<Unit>{let cell=Cell(future:fail());let receiver=receive(cell);return receiver;}async fn main(){let receiver=make();await receiver;}",
    );
}
