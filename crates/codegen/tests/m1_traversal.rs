#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use source_edit::ReplaceChecked;

use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};

#[test]
fn traversal_uses_concrete_schema_and_settles_worker_roots() {
    let source = SourceFile::new(
        FileId(1),
        "traversal.rinzu",
        include_str!("../../../tests/fixtures/programs/traversal/src/main.rinzu"),
    );
    let formatted = format_source(&source).unwrap();
    assert_eq!(
        formatted,
        format_source(&SourceFile::new(FileId(1), "traversal.rinzu", &formatted)).unwrap()
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"2\n2\n7\n[\"first\", \"second\"]\n7\n");
    assert_eq!(run.worker_heaps.len(), 1);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    assert_eq!(run.heap.objects, 0);

    let source = source
        .text()
        .replace_checked("async fn main()", "fn main()")
        .replace_checked(
            "let pending = spawn sumMap<String>(table);",
            "let answer = sumMap<String>(table);",
        )
        .replace_checked("await pending", "answer");
    let artifact =
        compile_source(&SourceFile::new(FileId(2), "traversal-gc.rinzu", source)).unwrap();
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
    assert_eq!(run.stdout, b"2\n2\n7\n[\"first\", \"second\"]\n7\n");
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 20);
}
