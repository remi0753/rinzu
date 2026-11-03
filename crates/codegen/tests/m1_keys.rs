#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use source_edit::ReplaceChecked;

use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn content_keys_survive_every_allocation_gc_and_four_worker_snapshots() {
    let text = include_str!("fixtures/m1-keys/src/main.rinzu");
    let artifact = compile_source(&SourceFile::new(FileId(1), "keys.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"keys ok\n");
    assert_eq!(run.worker_heaps.len(), 4);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    let text = text
        .replace_checked("async fn main()", "fn main()")
        .replace_checked("spawn echo<", "echo<")
        .replace_checked("await ", "");
    let artifact = compile_source(&SourceFile::new(FileId(2), "keys-gc.rinzu", text)).unwrap();
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
    assert!(gc.collection_count > 128);
}
