#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};
const SOURCE: &str = r#"
type Pair={x:Int,y:Int};
class Counter{value:Int}
fn original(counter:Counter)->Pair{counter.value=counter.value*10+1;return {x=10,y=20};}
fn next(counter:Counter,value:Int)->Int{counter.value=counter.value*10+value;return value;}
fn main(){
    let counter=Counter(value:0);
    let before:Pair={x=10,y=20};
    let after={...original(counter),y=next(counter,2),x=next(counter,3)};
    if counter.value!=123 || after.x!=3 || after.y!=2 || before.x!=10 {panic("evaluation order");}
    let holder={counter=counter,flag=false};
    let updated={...holder,flag=true};
    updated.counter.value=42;
    if holder.counter.value!=42 || holder.flag {panic("shallow snapshot");}
}
"#;
#[test]
fn update_evaluation_order_and_shallow_aliases_survive_gc() {
    let artifact =
        compile_source(&SourceFile::new(FileId(307), "record-spread.rinzu", SOURCE)).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.heap_objects, 0);
    let source = SOURCE.replace("fn main()", "async fn main()");
    let artifact = compile_source(&SourceFile::new(
        FileId(308),
        "record-spread-async.rinzu",
        source,
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(run.resources_returned);
}
