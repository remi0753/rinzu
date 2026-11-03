#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};

const SOURCE: &str = r#"
class Counter { value:Int }
class Saved { const read:fn()->Int }
fn values(counter:Counter)->List<Int>{counter.value=counter.value+1;return [1,2,3,4];}
fn sum(counter:Counter)->Int {
    var total=0;
    for item in values(counter) {
        try {
            if item==2 { continue; }
            if item==4 { break; }
            total=total+item;
        } finally { counter.value=counter.value+10; }
    }
    return total;
}
fn main(){
    let counter=Counter(value:0);
    if sum(counter)!=4 || counter.value!=41 {panic("control or evaluation order");}
    var source=[1,2,3];
    var snapshots:List<Saved> =[];
    for value in source {
        source=[9];
        let read=fn()->Int{return value;};
        snapshots=listAppend(snapshots,Saved(read:read));
    }
    var result=0;
    for saved in snapshots {result=result*10+saved.read();}
    if result!=123 {panic("capture snapshot");}
    for _ in source {}
    let empty:List<Int> =[];
    for absent in empty {panic("empty loop");}
}
"#;

#[test]
fn sync_and_async_paths_preserve_snapshot_control_and_gc_roots() {
    let artifact =
        compile_source(&SourceFile::new(FileId(302), "iteration.rinzu", SOURCE)).unwrap();
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
        FileId(303),
        "iteration-async.rinzu",
        source,
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(run.resources_returned);
}
