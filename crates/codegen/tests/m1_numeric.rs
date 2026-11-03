#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[test]
fn every_integer_width_and_float_boundary_vector_survives_gc() {
    let text = include_str!("fixtures/m1-numeric-vectors/src/main.rinzu");
    let artifact =
        compile_library(&SourceFile::new(FileId(3), "numeric-vectors.rinzu", text)).unwrap();
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
    assert_eq!(gc.stdout, b"numeric vectors ok\n");
    assert_eq!(gc.heap_objects, 0);
    assert!(gc.collection_count > 150);
}
#[test]
fn scalar_results_survive_gc_and_worker_transfer() {
    let text = include_str!("../../../tests/fixtures/programs/numeric/src/main.rinzu");
    let artifact = compile_library(&SourceFile::new(FileId(1), "numeric.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.worker_heaps.len(), 1);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    let text = text
        .replace("async fn main()", "fn main()")
        .replace("spawn echo(", "echo(")
        .replace("await pending", "pending");
    let artifact = compile_library(&SourceFile::new(FileId(2), "numeric-gc.rinzu", text)).unwrap();
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
}
