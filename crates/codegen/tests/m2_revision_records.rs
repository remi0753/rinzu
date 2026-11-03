#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn record_shapes_preserve_source_evaluation_order_copy_values_and_share_class_fields() {
    let text = include_str!("../../../tests/fixtures/programs/records/src/main.rinzu");
    let artifact = compile_source(&SourceFile::new(FileId(1), "records.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(run.stdout, b"true\ntrue\n1,2:1,10\n3\nfound\n321\n");
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.collections > 300);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    let text = text
        .replace("async fn main()", "fn main()")
        .replace("spawn snapshot(", "snapshot(")
        .replace("await pending", "pending");
    let artifact = compile_source(&SourceFile::new(FileId(2), "records-gc.rinzu", text)).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.stdout, b"true\ntrue\n1,2:1,10\n3\nfound\n321\n");
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 300);
}
