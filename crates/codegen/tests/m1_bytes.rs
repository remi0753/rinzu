#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "../../compiler/test-support/source_edit.rs"]
mod source_edit;
use source_edit::ReplaceChecked;

use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};

#[test]
fn dynamic_bytes_survive_moving_collection_and_worker_transfer() {
    let text = include_str!("../../../tests/fixtures/programs/bytes/src/main.rinzu");
    let source = SourceFile::new(FileId(1), "bytes.rinzu", text);
    let formatted = format_source(&source).unwrap();
    assert_eq!(
        formatted,
        format_source(&SourceFile::new(FileId(1), "bytes.rinzu", &formatted)).unwrap()
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
    assert_eq!(run.worker_heaps.len(), 1);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    assert_eq!(run.heap.objects, 0);
    let synchronous = text
        .replace_checked("async fn main()", "fn main()")
        .replace_checked(
            "let pending = spawn copy<List<ByteArray>>([combined, fixed]);",
            "let answer = copy<List<ByteArray>>([combined, fixed]);",
        )
        .replace_checked("await pending", "answer");
    let artifact =
        compile_source(&SourceFile::new(FileId(2), "bytes-gc.rinzu", synchronous)).unwrap();
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
    assert!(gc.collection_count > 20);
    assert_eq!(gc.heap_objects, 0);
}

#[test]
fn all_small_slice_intervals_and_short_chunks_survive_collection() {
    let text = include_str!("../../../tests/fixtures/programs/byte-chunks/src/main.rinzu");
    let text = text
        .replace_checked("async fn main()", "fn main()")
        .replace_checked(
            "let pending = spawn chunks(data);",
            "let pending = chunks(data);",
        )
        .replace_checked("await pending", "pending");
    let artifact = compile_source(&SourceFile::new(FileId(3), "byte-vectors.rinzu", text)).unwrap();
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
    assert!(
        run.stdout
            .starts_with(b"true\nOption<CUInt8>.None\nOption<ByteArray>.None\n")
    );
    assert!(run.stdout.ends_with(b"true\n"));
    assert!(run.collection_count > 100);
    assert_eq!(run.heap_objects, 0);
}

#[test]
fn byte_predicates_keep_boolean_values_and_argument_roots_through_jit_gc() {
    let source = SourceFile::new(
        FileId(4),
        "byte-predicates.rinzu",
        include_str!("fixtures/byte_scans.rinzu"),
    );
    let artifact = compile_source(&source).unwrap();
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
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 20);
}
