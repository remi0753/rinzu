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
fn error_values_and_caught_errors_survive_gc_in_the_same_domain() {
    let text = include_str!("../../../tests/fixtures/programs/errors/src/main.rinzu");
    let artifact = compile_library(&SourceFile::new(FileId(1), "errors.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.worker_heaps.is_empty());
    assert_eq!(run.heap.objects, 0);
    let text = text.replace_checked("async fn main()", "fn main()");
    let artifact = compile_library(&SourceFile::new(FileId(2), "errors-gc.rinzu", text)).unwrap();
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

#[test]
fn cancellation_bypasses_error_value_recovery_and_runs_cleanup() {
    let text = r#"
import rinzu.stdio;
async fn main() {
    try { var index=0; while index<100 {let ordinary=error(1000,"work");let result=errorWithCause(ordinary,error(1001,"cause"));index=index+1;} }
    catch [caught: Error, ...caughtRest] {rinzu.stdio.writeLine(errorCode(caught));}
    finally {rinzu.stdio.writeLine("cleanup");}
}
"#;
    let artifact =
        compile_library(&SourceFile::new(FileId(3), "error-cancel.rinzu", text)).unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            cancel_after_polls: Some(12),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, b"cleanup\n");
    assert!(run.caught_errors.is_empty());
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
