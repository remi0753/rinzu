#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};
const SOURCE: &str =
    include_str!("../../../tests/fixtures/programs/language-evolution/src/main.rinzu");

#[test]
fn interface_dispatch_and_error_contracts_execute_with_gc_and_async_cleanup() {
    let source = r#"
        error Failed{}
        interface Getter<T>{fn get(self:Self)->T nothrow;}
        class Box<T> interface Getter<T>{private const value:T
            fn create(value:T)->Box<T> nothrow{return Box(value:value);}
            fn get(self:Box<T>)->T nothrow{let read=fn()->T nothrow{return self.value;};return read();}
        }
        enum Number interface Getter<Int>{Empty,Value(value:Int),
            fn get(self:Number)->Int nothrow{switch self{case Number.Empty:{return 0;}case Number.Value(value):{return value;}}}
        }
        fn unbox<T,B interface Getter<T>>(box:B,otherwise:T)->T nothrow{return box.get();}
        fn read<G interface Getter<Int>>(value:G)->Int nothrow{let get=G.get;return get(value);}
        fn synchronous()->Int nothrow{
            let box=Box.create(42);
            if unbox(Number.Value(value:42),0)!=unbox(box,0){panic("interface dispatch");}
            return read(box);
        }
        interface AsyncValue{async fn get(self:Self)->Int throws Failed;}
        class Ready interface AsyncValue{
            async fn get(self:Ready)->Int nothrow{return 42;}
        }
        class Broken interface AsyncValue{
            async fn get(self:Broken)->Int throws Failed{throw Failed(message:"expected");}
        }
        async fn observe<R interface AsyncValue>(reader:R)->Int throws Failed{return await reader.get();}
        async fn main()nothrow{
            if synchronous()!=42{panic("sync result");}
            try{if await observe(Ready())!=42{panic("async result");}}catch errors{panic("unexpected failure");}
            var caught=false;
            try{let value=await observe(Broken());}catch [error:Failed,...rest]{caught=true;}finally{let cleanupValue=synchronous();}
            if !caught{panic("missing failure");}
        }
    "#;
    let artifact = compile_source(&SourceFile::new(
        FileId(313),
        "interfaces-contracts.rinzu",
        source,
    ))
    .unwrap();
    let sync = run_baseline_jit_with_options(
        &artifact.ir,
        "synchronous",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(sync.result, Ok(42));
    assert_eq!(sync.heap_objects, 0);
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
#[test]
fn approved_features_compose_in_sync_jit_and_async_managed_execution() {
    let sync = SOURCE
        .split("// async scenarios")
        .next()
        .unwrap()
        .to_owned()
        + "fn main(){if synchronous()!=42{panic(\"wrong result\");}}";
    let artifact = compile_source(&SourceFile::new(
        FileId(311),
        "language-evolution-sync.rinzu",
        sync,
    ))
    .unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "synchronous",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(42));
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 5);
    let artifact = compile_source(&SourceFile::new(
        FileId(312),
        "language-evolution.rinzu",
        SOURCE,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
