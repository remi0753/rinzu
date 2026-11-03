use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Lost{}
async fn fail(){throw Lost(message:"failed");}
async fn safe(){}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
class Cell{future:Future<Unit>}
fn build()->Cell{return Cell(future:fail());}
fn read(cell:Cell)->Future<Unit>{return cell.future;}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(264),
        "future-objects.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let errors = compile_source(&SourceFile::new(
        FileId(264),
        "future-objects.rinzu",
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
fn local_class_fields_and_aliases_retain_the_same_future() {
    accepted(
        "fn main(){let cell=Cell(future:fail());let alias=cell;let receiver=observe(alias.future);}",
    );
    lost("fn main(){let cell=Cell(future:fail());let alias=cell;}");
}
#[test]
fn a_synchronous_factory_and_readonly_getter_preserve_identity() {
    accepted("fn main(){let cell=build();let receiver=observe(read(cell));}");
    lost("fn main(){let cell=build();}");
}
#[test]
fn overwriting_through_an_alias_does_not_observe_the_old_field() {
    lost(
        "fn main(){let cell=build();let alias=cell;alias.future=safe();let receiver=observe(cell.future);}",
    );
    accepted(
        "fn main(){let cell=build();let saved=cell.future;let alias=cell;alias.future=fail();let first=observe(saved);let second=observe(cell.future);}",
    );
}
#[test]
fn observing_one_instance_does_not_observe_another() {
    lost("fn main(){let first=build();let second=build();let receiver=observe(first.future);}");
    accepted(
        "fn main(){let first=build();let second=build();let firstReceiver=observe(first.future);let secondReceiver=observe(second.future);}",
    );
}
#[test]
fn an_error_before_the_field_read_keeps_the_obligation() {
    lost(
        "fn main(){try{let cell=build();let invalid=CInt8(300);let receiver=observe(cell.future);}catch errors{}}",
    );
}
#[test]
fn only_the_last_instance_of_repeated_allocations_cannot_cover_the_family() {
    lost(
        "fn make(count:Int)->Cell{var last=build();var index=0;while index<count{last=build();index=index+1;}return last;}fn main(){let receiver=observe(read(make(3)));}",
    );
}
#[test]
fn function_writes_cannot_leave_a_stale_observation_receipt() {
    lost(
        "fn replace(cell:Cell){cell.future=safe();}fn main(){let cell=build();replace(cell);let receiver=observe(read(cell));}",
    );
    lost(
        "fn replaceAndRead(left:Cell,right:Cell){left.future=safe();let receiver=observe(right.future);}fn main(){let cell=build();replaceAndRead(cell,cell);}",
    );
}
#[test]
fn a_stored_future_can_be_snapshotted_before_an_async_mutation() {
    accepted(
        "async fn replace(cell:Cell){cell.future=safe();}fn main(){let cell=build();let saved=cell.future;let writer=replace(cell);let receiver=observe(saved);}",
    );
    lost(
        "async fn replace(cell:Cell){cell.future=safe();}fn main(){let cell=build();let writer=replace(cell);cell.future=safe();let receiver=observe(cell.future);}",
    );
}

#[test]
fn an_async_writer_invalidates_scalar_guards_as_well_as_future_fields() {
    lost(
        "class Flag{enabled:Bool}async fn flip(flag:Flag){flag.enabled=false;}fn main(){let pending=fail();let flag=Flag(enabled:true);let writer=flip(flag);if flag.enabled{let receiver=observe(pending);}}",
    );
}

#[test]
fn a_native_callback_cannot_leave_stale_scalar_guards() {
    lost(
        "native{fn invoke(handler:fn(Int)->Int)->Int symbol \"invoke\";}class Flag{enabled:Bool}fn main(){let pending=fail();let flag=Flag(enabled:true);let result=invoke(fn(value:Int)->Int fallback 0{flag.enabled=false;return value;});if flag.enabled{let receiver=observe(pending);}}",
    );
}

#[test]
fn a_sync_wrapper_does_not_make_async_field_observation_a_snapshot() {
    lost(
        "async fn receive(cell:Cell){try{await cell.future;}catch errors{}}fn schedule(cell:Cell){let receiver=receive(cell);}fn main(){let cell=build();schedule(cell);cell.future=safe();}",
    );
    lost(
        "type Packet={cell:Cell};async fn receive(packet:Packet){try{await packet.cell.future;}catch errors{}}fn schedule(packet:Packet){let receiver=receive(packet);}fn main(){let cell=build();schedule({cell=cell});cell.future=safe();}",
    );
}

#[test]
fn an_async_writer_stays_pending_through_a_sync_wrapper() {
    lost(
        "class Flag{enabled:Bool}async fn flip(flag:Flag){flag.enabled=false;}fn schedule(flag:Flag){let writer=flip(flag);}fn main(){let pending=fail();let flag=Flag(enabled:false);schedule(flag);flag.enabled=true;if flag.enabled{let receiver=observe(pending);}}",
    );
}

#[test]
fn returned_aliases_are_one_instance_and_reads_before_writes_keep_the_old_future() {
    let declaration = "type Pair={left:Cell,right:Cell};fn pair()->Pair{let cell=build();return {left=cell,right=cell};}";
    lost(&format!(
        "{declaration}fn main(){{let aliases=pair();aliases.left.future=safe();let receiver=observe(aliases.right.future);}}"
    ));
    accepted(&format!(
        "{declaration}fn main(){{let aliases=pair();let saved=aliases.left.future;aliases.left.future=fail();let first=observe(saved);let second=observe(aliases.right.future);}}"
    ));
}

#[test]
fn cyclic_instances_and_per_iteration_observation_reach_a_fixed_point() {
    accepted(
        "class Node{future:Future<Unit>,next:Option<Node>}fn make()->Node{let node=Node(future:fail(),next:Option<Node>.None());node.next=Option<Node>.Some(value:node);return node;}fn main(){let node=make();let receiver=observe(node.future);}",
    );
    accepted(
        "fn main(){var index=0;while index<5{let cell=build();let receiver=observe(cell.future);index=index+1;}}",
    );
}
