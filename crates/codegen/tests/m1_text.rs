#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../test-support/library.rs"]
mod test_library;
use test_library::compile_library;
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use source_edit::ReplaceChecked;

use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile};

#[test]
fn all_text_boundary_vectors_survive_moving_collection() {
    let text = include_str!("fixtures/m1-text-vectors/src/main.rinzu")
        .replace_checked("async fn main()", "fn main()")
        .replace_checked("spawn echo(", "echo(")
        .replace_checked("await pending", "pending");
    let artifact =
        compile_library(&SourceFile::new(FileId(3), "text-vectors.rinzu", text)).unwrap();
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
    assert_eq!(gc.stdout, b"text vectors ok\n");
    assert_eq!(gc.heap_objects, 0);
    assert!(gc.collection_count > 500);
}
#[test]
fn text_results_survive_gc_and_worker_transfer() {
    let text = include_str!("../../../tests/fixtures/programs/text/src/main.rinzu");
    let artifact = compile_library(&SourceFile::new(FileId(1), "text.rinzu", text)).unwrap();
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
        .replace_checked("async fn main()", "fn main()")
        .replace_checked("spawn echo<", "echo<")
        .replace_checked("await pending", "pending");
    let artifact = compile_library(&SourceFile::new(FileId(2), "text-gc.rinzu", text)).unwrap();
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
