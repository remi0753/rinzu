use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn synchronous_helpers_check_unbound_callees_before_arguments_and_preserve_recoverable_errors() {
    let artifact = compile_source(&SourceFile::new(
        FileId(249),
        "normal-error-origin-execution.rinzu",
        include_str!("fixtures/normal-error-origins.rinzu"),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:#?}");
    assert_eq!(run.stdout, b"73\n79\n83\n");
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}
