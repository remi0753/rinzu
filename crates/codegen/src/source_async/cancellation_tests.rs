use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};

const WORKER: &str = r#"
fn work() -> Int {
    try { var value = 0; while true { value = value + 1; } return value; }
    finally {
        try { let local = CInt8(300); } catch [inside: Error, ...insideRest] { let message = inside.message; }
        try { let cleanup = CInt8(400); } finally { let other = CInt8(500); }
    }
}
"#;
const MAIN: &str = r#"
async fn main() {
    try { let value = await spawn work(); }
    catch wrong { let should_not_catch = CInt8(900); }
    finally { let cleanup = CInt8(600); }
}
"#;

#[test]
fn worker_cancellation_preserves_primary_and_runs_worker_then_origin_cleanup() {
    let source = SourceFile::new(FileId(51), "cancel-worker.rinzu", format!("{WORKER}{MAIN}"));
    let artifact = compile_source(&source).unwrap();
    for limit in [2, 3, 7] {
        let worker = crate::run_baseline_jit_with_options(
            &artifact.ir,
            "work",
            crate::BaselineOptions {
                cancel_after_polls: Some(limit),
                ..crate::BaselineOptions::default()
            },
        )
        .unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                cancel_worker_after_polls: Some(limit),
                ..SourceAsyncOptions::default()
            },
        )
        .unwrap();
        let failure = run.failure_details.as_ref().unwrap();
        assert_eq!(
            failure.primary,
            worker.failure_details.as_ref().unwrap().primary
        );
        assert_eq!(failure.primary.cause, FailureCause::Cancelled);
        assert_eq!(failure.cleanup.len(), 3);
        assert_eq!(
            failure.cleanup[..2],
            worker.failure_details.unwrap().cleanup
        );
        assert_eq!(run.caught_errors, worker.caught_errors);
        assert_eq!(run.transport.peak_transport_bytes, 104);
        assert!(run.resources_returned);
    }
}

#[test]
fn cancellation_winner_waits_for_worker_resources_and_origin_finally() {
    let source = SourceFile::new(FileId(52), "cancel-winner.rinzu", format!("{WORKER}{MAIN}"));
    let artifact = compile_source(&source).unwrap();
    for _ in 0..16 {
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                cancel_first_spawn: true,
                ..SourceAsyncOptions::default()
            },
        )
        .unwrap();
        let failure = run.failure_details.unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Cancelled);
        assert!(failure.primary.source.is_some());
        let cleanup = failure.cleanup.last().unwrap();
        assert_eq!(cleanup.cause, FailureCause::Error(ErrorKind::Conversion));
        assert!(cleanup.source.unwrap().start_byte > failure.primary.source.unwrap().start_byte);
        assert!(
            run.events
                .iter()
                .any(|event| matches!(event, SourceAsyncEvent::Cancelled { .. }))
        );
        assert!(run.resources_returned);
    }
}
