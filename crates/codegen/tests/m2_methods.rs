#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};
#[test]
fn receiver_and_argument_order_explicit_getter_and_bound_cycle_survive_every_gc() {
    let text = include_str!("../../../tests/fixtures/methods/order.rinzu");
    let artifact = compile_source(&SourceFile::new(FileId(1), "method-order.rinzu", text)).unwrap();
    let result = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result.result, Ok(0));
    assert_eq!(result.stdout, b"ABCD,42,42,42,45,48,48,8,1,8\n");
    assert!(result.collection_count > 400);
    assert_eq!(result.heap_objects, 0);
}
#[test]
fn async_and_worker_methods_return_all_domain_roots() {
    let text = include_str!("../../../tests/fixtures/methods/async.rinzu");
    let artifact = compile_source(&SourceFile::new(FileId(1), "method-async.rinzu", text)).unwrap();
    let result = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.failure_details.is_none(), "{result:?}");
    assert!(result.resources_returned);
    assert_eq!(result.stdout, b"42,42\n");
    assert_eq!((result.heap.objects, result.heap.roots), (0, 0));
    assert!(
        result
            .worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
}
