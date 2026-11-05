use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
import rinzu.list;
import rinzu.map;
error Lost{} async fn fail(){throw Lost(message:"failure");}
async fn observe(value:Future<Unit>){try{await value;}catch errors{}}
async fn observeMap(values:Map<String,Future<Unit>>) {
 let keys=rinzu.map.keys(values);
 var index=0;
 while index<rinzu.list.length(keys) {
  switch rinzu.list.get(keys,index) {
   case Option.Some(key): {switch rinzu.map.get(values,key){case Option.Some(value):{let receiver=observe(value);}case Option.None:{}}}
   case Option.None: {}
  }
  index=index+1;
 }
}
"#;
fn accepted(source: &str) {
    compile_source(&SourceFile::new(
        FileId(261),
        "future-maps.rinzu",
        format!("{BASE}{source}"),
    ))
    .unwrap();
}
fn lost(source: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(261),
        "future-maps.rinzu",
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
fn all_map_values_can_be_handed_to_independent_observers() {
    accepted("fn main(){let receiver=observeMap({\"a\":fail(),\"b\":fail()});}");
}

#[test]
fn known_overwrite_and_removal_leave_the_original_tasks_responsibility() {
    lost(
        "fn main(){let first=fail();let second=fail();let values=rinzu.map.insert({\"key\":first},\"key\",second);let receiver=observeMap(values);}",
    );
    accepted(
        "async fn main(){let first=fail();let second=fail();let values=rinzu.map.insert({\"key\":first},\"key\",second);let receiver=observeMap(values);try{await first;}catch[Lost]{}}",
    );
    lost(
        "fn main(){let first=fail();let values=rinzu.map.remove({\"key\":first},\"key\");let receiver=observeMap(values);}",
    );
    accepted(
        "fn main(){let first=fail();let original={\"key\":first};let changed=rinzu.map.remove(original,\"key\");let receiver=observeMap(original);}",
    );
}

#[test]
fn dynamic_key_collisions_do_not_guarantee_the_earlier_values_are_retained() {
    lost(
        "fn run(firstKey:String,secondKey:String){let first=fail();let second=fail();let values={firstKey:first,secondKey:second};let receiver=observeMap(values);}fn main(){run(\"same\",\"same\");}",
    );
    accepted(
        "async fn run(firstKey:String,secondKey:String){let first=fail();let second=fail();let values={firstKey:first,secondKey:second};let receiver=observeMap(values);try{await first;}catch[Lost]{}}async fn main(){await run(\"same\",\"same\");}",
    );
}

#[test]
fn keys_are_ordinary_values_and_do_not_transfer_future_responsibility() {
    lost(
        "fn make()->List<String>{let values={\"key\":fail()};return rinzu.map.keys(values);}fn main(){let keys=make();}",
    );
    lost("fn main(){let values={\"key\":fail()};let keys=rinzu.map.keys(values);}");
}

#[test]
fn a_different_map_cannot_use_another_maps_key_snapshot_to_claim_all_values() {
    let wrong = BASE[BASE.find("async fn observeMap").unwrap()..]
        .replace(
            "observeMap(values:Map<String,Future<Unit>>)",
            "wrong(keysFrom:Map<String,Future<Unit>>,values:Map<String,Future<Unit>>)",
        )
        .replace(
            "let keys=rinzu.map.keys(values)",
            "let keys=rinzu.map.keys(keysFrom)",
        );
    lost(&format!(
        "{wrong} fn main(){{let receiver=wrong({{\"a\":fail()}},{{\"b\":fail()}});}}"
    ));
}

#[test]
fn an_alias_of_the_keys_length_keeps_the_same_snapshot() {
    let observer = BASE[BASE.find("async fn observeMap").unwrap()..]
        .replace("observeMap(", "withLength(")
        .replace(
            "var index=0;",
            "let count=rinzu.list.length(keys);var index=0;",
        )
        .replace("index<rinzu.list.length(keys)", "index<count");
    accepted(&format!(
        "{observer} fn main(){{let receiver=withLength({{\"a\":fail(),\"b\":fail()}});}}"
    ));
}

#[test]
fn a_known_map_read_returns_the_replacement_not_the_overwritten_future() {
    lost(
        "async fn main(){let first=fail();let second=fail();let values=rinzu.map.insert({\"key\":first},\"key\",second);switch rinzu.map.get(values,\"key\"){case Option.Some(value):{try{await value;}catch[Lost]{}}case Option.None:{}}try{await second;}catch[Lost]{}}",
    );
}

#[test]
fn an_evaluated_duplicate_literal_key_keeps_only_the_later_value() {
    lost(
        "fn main(){let key=\"key\";let first=fail();let second=fail();let values={key:first,\"key\":second};let receiver=observeMap(values);}",
    );
    accepted(
        "async fn main(){let key=\"key\";let first=fail();let second=fail();let values={key:first,\"key\":second};let receiver=observeMap(values);try{await first;}catch[Lost]{}}",
    );
}
