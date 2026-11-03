use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_compiler::{FileId, SourceFile, compile_source};
use rinzu_runtime::failure::FailureCause;

const HELPERS: &str = r#"
import rinzu.stdio;
fn line(text:String){try{rinzu.stdio.writeLine(text);}catch errors{}}
fn fail(){
    try all {
        panic("assertion failed 日本語🙂\u{0}tail");
        line("unreachable");
    } catch errors {
        line("caught");
    } finally {
        line("cleanup");
    }
}
"#;

#[test]
fn panic_bypasses_catch_and_try_all_but_runs_finally_on_main_and_worker() {
    for entry in [
        "async fn main(){let status=setProcessExitStatus(0);fail();}",
        "async fn main(){await spawn fail();}",
        "async fn child(){fail();}async fn main(){child();}",
    ] {
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "panic.rinzu",
            format!("{HELPERS}{entry}"),
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
        let failure = run.failure_details.as_ref().unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Panic, "{run:#?}");
        assert!(failure.primary.source.is_some());
        assert!(!failure.recoverable());
        assert_eq!(run.exit_status, 1);
        assert_eq!(run.stdout, b"cleanup\n");
        assert!(run.resources_returned, "{run:#?}");
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    }
}

#[test]
fn a_fatal_panic_cancels_and_joins_outstanding_fallible_tasks() {
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "panic-pending.rinzu",
        "error Failed{} async fn fail(){throw Failed(message:\"background\");} async fn main(){let pending=fail();panic(\"fatal\");}",
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        run.failure_details.as_ref().unwrap().primary.cause,
        FailureCause::Panic
    );
    assert_eq!(run.exit_status, 1);
    assert!(run.resources_returned, "{run:#?}");
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
}

#[test]
fn a_cleanup_panic_preserves_the_primary_panic_and_ordered_cleanup() {
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "panic-cleanup.rinzu",
        "async fn main(){try{try{panic(\"primary\");}finally{panic(\"cleanup\");}}catch errors{}}",
    ))
    .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    let failure = run.failure_details.as_ref().unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Panic);
    assert_eq!(failure.cleanup.len(), 1);
    assert_eq!(failure.cleanup[0].cause, FailureCause::Panic);
    assert!(
        failure.primary.source.unwrap().start_byte < failure.cleanup[0].source.unwrap().start_byte
    );
    assert_eq!(run.exit_status, 1);
    assert!(run.resources_returned);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn synchronous_jit_panic_preserves_finally_and_releases_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "panic.rinzu",
        format!("{HELPERS}fn main(){{fail();}}"),
    ))
    .unwrap();
    let run = rinzu_codegen::run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        rinzu_codegen::BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Err(rinzu_codegen::BaselineFailure::Panic));
    assert_eq!(run.stdout, b"cleanup\n");
    assert_eq!(run.heap_objects, 0);
    assert!(run.caught_errors.is_empty());
}
