use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Lost{}
async fn fail(){throw Lost(message:"failure");}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
async fn observeMap(values:Map<Int,Future<Unit>>){
 let keys=mapKeys(values);var index=0;
 while index<listLength(keys){
  switch listGet(keys,index){
   case Option.Some(key):{switch mapGet(values,key){case Option.Some(value):{let receiver=observe(value);}case Option.None:{}}}
   case Option.None:{}
  }
  index=index+1;
 }
}
"#;
const BUILDER: &str = r#"
fn build(count:Int)->Map<Int,Future<Unit>>{
 var output:Map<Int,Future<Unit>> = {};
 var index=0;
 while index<count{output=mapInsert(output,index,fail());index=index+1;}
 return output;
}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(262),
        "future-growing-maps.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let errors = compile_source(&SourceFile::new(
        FileId(262),
        "future-growing-maps.rinzu",
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
fn noncolliding_counter_keys_retain_every_pending_task() {
    accepted(&format!(
        "{BUILDER}fn main(){{let receiver=observeMap(build(5));}}"
    ));
    lost(&format!("{BUILDER}fn main(){{let values=build(5);}}"));
}
#[test]
fn repeated_or_unknown_keys_do_not_transfer_overwritten_tasks() {
    for key in ["0", "count"] {
        let builder = BUILDER.replace(
            "mapInsert(output,index,fail())",
            &format!("mapInsert(output,{key},fail())"),
        );
        lost(&format!(
            "{builder}fn main(){{let receiver=observeMap(build(5));}}"
        ));
    }
}
#[test]
fn resets_and_extra_writes_invalidate_counter_key_uniqueness() {
    for update in ["index=index+0;", "index=0;index=index+1;"] {
        let builder = BUILDER.replace("index=index+1;", update);
        lost(&format!(
            "{builder}fn main(){{let receiver=observeMap(build(5));}}"
        ));
    }
    let builder = BUILDER.replace(
        "index=index+1;",
        "output=mapInsert(output,0,fail());index=index+1;",
    );
    lost(&format!(
        "{builder}fn main(){{let receiver=observeMap(build(5));}}"
    ));
}
#[test]
fn removal_and_dropping_the_map_leave_a_responsibility() {
    let dropped = BUILDER.replace("return output;", "return {};");
    lost(&format!(
        "{dropped}fn main(){{let receiver=observeMap(build(5));}}"
    ));
    lost(&format!(
        "{BUILDER}fn main(){{let values=build(5);let changed=mapRemove(values,0);let receiver=observeMap(changed);}}"
    ));
    accepted(&format!(
        "{BUILDER}fn main(){{let values=build(5);let changed=mapRemove(values,0);let receiver=observeMap(values);}}"
    ));
}
#[test]
fn an_error_before_storage_cannot_lose_the_created_task() {
    let interrupted = BUILDER.replace(
        "output=mapInsert(output,index,fail());",
        "let pending=fail();let invalid=CInt8(300);output=mapInsert(output,index,pending);",
    );
    lost(&format!(
        "{interrupted}fn main(){{try{{let receiver=observeMap(build(5));}}catch errors{{}}}}"
    ));
}
#[test]
fn distinct_returned_maps_and_stale_aliases_keep_separate_responsibilities() {
    lost(&format!(
        "{BUILDER}fn main(){{let first=build(3);let second=build(3);let receiver=observeMap(first);}}"
    ));
    accepted(&format!(
        "{BUILDER}fn main(){{let first=build(3);let second=build(3);let firstReceiver=observeMap(first);let secondReceiver=observeMap(second);}}"
    ));
}

#[test]
fn complete_aliases_transfer_the_map_but_keys_alone_do_not() {
    let aliased = BUILDER.replace("return output;", "let saved=output;output={};return saved;");
    accepted(&format!(
        "{aliased}fn main(){{let receiver=observeMap(build(3));}}"
    ));
    let keys = BUILDER
        .replace("->Map<Int,Future<Unit>>", "->List<Int>")
        .replace("return output;", "return mapKeys(output);");
    lost(&format!("{keys}fn main(){{let keys=build(3);}}"));
}

#[test]
fn rotated_batches_must_observe_each_new_generation() {
    accepted(&format!(
        "{BUILDER}fn main(){{var previous:Map<Int,Future<Unit>> = {{}};var index=0;while index<3{{let current=build(2);let receiver=observeMap(previous);previous=current;index=index+1;}}let finalReceiver=observeMap(previous);}}"
    ));
    lost(&format!(
        "{BUILDER}fn main(){{var old:Map<Int,Future<Unit>> = {{}};var index=0;while index<3{{let current=build(2);if index==0{{let receiver=observeMap(current);old=current;}}else{{let stale=observeMap(old);}}index=index+1;}}}}"
    ));
}
