#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};
const SOURCE: &str =
    include_str!("../../../tests/fixtures/programs/language-evolution/src/main.rinzu");
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
