#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};
#[test]
fn constant_roots_survive_gc_and_domain_copy_with_generic_uses() {
    let text = include_str!("../../../tests/fixtures/programs/constants/src/main.rinzu");
    let artifact = compile_source(&SourceFile::new(FileId(1), "constants.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    assert_eq!(run.worker_heaps.len(), 1);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    let text = text
        .replace("async fn main()", "fn main()")
        .replace("spawn copy(", "copy(")
        .replace("await pending", "pending");
    let artifact = compile_source(&SourceFile::new(FileId(2), "const-gc.rinzu", text)).unwrap();
    let gc = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(gc.result, Ok(0));
    assert_eq!(gc.stdout, run.stdout);
    assert_eq!(gc.heap_objects, 0);
    assert!(gc.collection_count > 20);
}
