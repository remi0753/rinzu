use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Lost{}
async fn fail(){throw Lost(message:"failure");}
async fn safe(){}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
class Cell{future:Future<Unit>}
async fn assign(cell:Cell,value:Future<Unit>){cell.future=value;}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(270),
        "future-async-fields.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let errors = compile_source(&SourceFile::new(
        FileId(270),
        "future-async-fields.rinzu",
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
fn an_awaited_assignment_exposes_the_stored_future() {
    accepted(
        "async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=assign(cell,pending);await writer;let receiver=observe(cell.future);}",
    );
    lost(
        "fn main(){let pending=fail();let cell=Cell(future:safe());let writer=assign(cell,pending);}",
    );
}
#[test]
fn a_callers_intervening_store_does_not_reveal_the_writers_old_final_value() {
    lost(
        "async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=assign(cell,pending);cell.future=safe();await writer;let receiver=observe(cell.future);}",
    );
    accepted(
        "async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=assign(cell,pending);cell.future=safe();await writer;let independent=observe(pending);}",
    );
}
#[test]
fn sequential_writers_can_be_observed_between_assignments() {
    accepted(
        "async fn main(){let first=fail();let second=fail();let cell=Cell(future:safe());let one=assign(cell,first);await one;let before=observe(cell.future);let two=assign(cell,second);await two;let after=observe(cell.future);}",
    );
}
#[test]
fn overlapping_writers_cannot_each_claim_the_same_final_field() {
    lost(
        "async fn main(){let first=fail();let second=fail();let cell=Cell(future:safe());let one=assign(cell,first);let two=assign(cell,second);await one;let before=observe(cell.future);await two;let after=observe(cell.future);}",
    );
}
#[test]
fn a_body_error_after_the_store_still_publishes_the_completed_write() {
    accepted(
        "async fn assignThenFail(cell:Cell,value:Future<Unit>){cell.future=value;let invalid=CInt8(300);}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=assignThenFail(cell,pending);try{await writer;}catch errors{}let receiver=observe(cell.future);}",
    );
}
#[test]
fn a_task_created_in_an_async_body_can_be_received_after_its_store_completes() {
    accepted(
        "async fn create(cell:Cell){cell.future=fail();}async fn main(){let cell=Cell(future:safe());let writer=create(cell);await writer;let receiver=observe(cell.future);}",
    );
    lost(
        "async fn create(cell:Cell){cell.future=fail();}fn main(){let cell=Cell(future:safe());let writer=create(cell);}",
    );
}

#[test]
fn repeated_await_does_not_reapply_a_completed_writers_snapshot() {
    lost(
        "async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=assign(cell,pending);await writer;cell.future=safe();await writer;let receiver=observe(cell.future);}",
    );
    accepted(
        "async fn main(){let first=fail();let second=fail();let cell=Cell(future:safe());let writer=assign(cell,first);await writer;let before=observe(cell.future);cell.future=second;await writer;let after=observe(cell.future);}",
    );
}

#[test]
fn a_stable_record_path_can_identify_the_target_cell() {
    accepted(
        "type Packet={cell:Cell};async fn update(packet:Packet,value:Future<Unit>){packet.cell.future=value;}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update({cell=cell},pending);await writer;let receiver=observe(cell.future);}",
    );
}

#[test]
fn a_sync_wrapper_can_return_the_writer_without_claiming_an_early_store() {
    accepted(
        "fn start(cell:Cell,value:Future<Unit>)->Future<Unit>{return assign(cell,value);}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=start(cell,pending);await writer;let receiver=observe(cell.future);}",
    );
    lost(
        "fn start(cell:Cell,value:Future<Unit>){let writer=assign(cell,value);}fn main(){let pending=fail();let cell=Cell(future:safe());start(cell,pending);let receiver=observe(cell.future);}",
    );
}

#[test]
fn a_factory_returns_both_the_write_handle_and_its_storage() {
    accepted(
        "type Pending={cell:Cell,writer:Future<Unit>};fn start()->Pending{let cell=Cell(future:safe());let writer=assign(cell,fail());return {cell=cell,writer=writer};}async fn main(){let result=start();await result.writer;let receiver=observe(result.cell.future);}",
    );
    lost(
        "fn start()->Future<Unit>{let cell=Cell(future:safe());let writer=assign(cell,fail());return writer;}async fn main(){let writer=start();await writer;}",
    );
}

#[test]
fn every_writer_outcome_must_retain_a_preexisting_obligation() {
    lost(
        "type Pending={cell:Cell,writer:Future<Unit>};async fn maybeAssign(cell:Cell,value:Future<Unit>,flag:Bool){if flag{cell.future=value;}}fn start(flag:Bool)->Pending{let cell=Cell(future:safe());let value=fail();let writer=maybeAssign(cell,value,flag);return {cell=cell,writer=writer};}async fn main(){let result=start(false);await result.writer;let receiver=observe(result.cell.future);}",
    );
    accepted(
        "type Pending={cell:Cell,writer:Future<Unit>,original:Future<Unit>};async fn maybeAssign(cell:Cell,value:Future<Unit>,flag:Bool){if flag{cell.future=value;}}fn start(flag:Bool)->Pending{let cell=Cell(future:safe());let value=fail();let writer=maybeAssign(cell,value,flag);return {cell=cell,writer=writer,original=value};}async fn main(){let result=start(false);await result.writer;let receiver=observe(result.original);}",
    );
}

#[test]
fn aliased_async_inputs_do_not_reorder_the_final_stores() {
    lost(
        "async fn update(right:Cell,left:Cell,value:Future<Unit>){left.future=value;right.future=safe();}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,cell,pending);await writer;let receiver=observe(cell.future);}",
    );
    accepted(
        "async fn update(right:Cell,left:Cell,value:Future<Unit>){left.future=value;right.future=safe();}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,cell,pending);await writer;let independent=observe(pending);}",
    );
}

#[test]
fn an_outer_await_can_publish_a_completed_nested_writer() {
    accepted(
        "async fn update(cell:Cell,value:Future<Unit>){let nested=assign(cell,value);await nested;}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,pending);await writer;let receiver=observe(cell.future);}",
    );
    lost(
        "async fn update(cell:Cell,value:Future<Unit>){let nested=assign(cell,value);}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,pending);await writer;let receiver=observe(cell.future);}",
    );
}

#[test]
fn a_nested_intervening_store_invalidates_the_older_completion() {
    lost(
        "async fn update(cell:Cell,value:Future<Unit>){let nested=assign(cell,value);cell.future=safe();await nested;}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,pending);await writer;let receiver=observe(cell.future);}",
    );
    accepted(
        "async fn update(cell:Cell,value:Future<Unit>){let nested=assign(cell,value);cell.future=safe();await nested;}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,pending);await writer;let independent=observe(pending);}",
    );
}

#[test]
fn a_returned_child_writer_can_finish_after_its_parent() {
    accepted(
        "type Started={writer:Future<Unit>};async fn start(cell:Cell,value:Future<Unit>)->Started{let child=assign(cell,value);return {writer=child};}async fn main(){let pending=fail();let cell=Cell(future:safe());let parent=start(cell,pending);let result=await parent;await result.writer;let receiver=observe(cell.future);}",
    );
    lost(
        "type Started={writer:Future<Unit>};async fn start(cell:Cell,value:Future<Unit>)->Started{let child=assign(cell,value);return {writer=child};}async fn main(){let pending=fail();let cell=Cell(future:safe());let parent=start(cell,pending);let result=await parent;let receiver=observe(cell.future);}",
    );
}

#[test]
fn exported_child_mutations_remain_active_across_outcome_alternatives() {
    lost(
        "type Started={writer:Future<Unit>,flag:Bool};async fn start(cell:Cell,value:Future<Unit>,flag:Bool)->Started{let child=assign(cell,value);if flag{return {writer=child,flag=true};}return {writer=child,flag=false};}async fn main(){let pending=fail();let cell=Cell(future:safe());let parent=start(cell,pending,true);cell.future=safe();let result=await parent;await result.writer;let receiver=observe(cell.future);}",
    );
    accepted(
        "type Started={writer:Future<Unit>,flag:Bool};async fn start(cell:Cell,value:Future<Unit>,flag:Bool)->Started{let child=assign(cell,value);if flag{return {writer=child,flag=true};}return {writer=child,flag=false};}async fn main(){let pending=fail();let cell=Cell(future:safe());let parent=start(cell,pending,true);let result=await parent;await result.writer;let receiver=observe(cell.future);}",
    );
}

#[test]
fn nested_writers_preserve_alias_and_overlap_invalidations() {
    lost(
        "async fn update(left:Cell,right:Cell,value:Future<Unit>){let nested=assign(left,value);right.future=safe();await nested;}async fn main(){let pending=fail();let cell=Cell(future:safe());let writer=update(cell,cell,pending);await writer;let receiver=observe(cell.future);}",
    );
    lost(
        "async fn update(cell:Cell,first:Future<Unit>,second:Future<Unit>){let one=assign(cell,first);let two=assign(cell,second);await one;await two;}async fn main(){let first=fail();let second=fail();let cell=Cell(future:safe());let writer=update(cell,first,second);await writer;let receiver=observe(cell.future);}",
    );
}
