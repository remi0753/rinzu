use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};

#[test]
fn successful_return_break_and_continue_run_finally_and_keep_return_roots() {
    let source = SourceFile::new(
        FileId(190),
        "return-finally.rinzu",
        r#"
fn returning() -> String {
    try { try { return "retained"; }
        finally { try { let badInner = CInt8(300); } catch [inner: Error, ...innerRest] { let copiedInner = inner.message; } }
    } finally { try { let badOuter = CInt8(400); } catch [outer: Error, ...outerRest] { let copiedOuter = outer.message; } }
}
async fn ready() -> String {
    try { return "retained"; }
    finally { try { let badReady = CInt8(300); } catch [readyError: Error, ...readyErrorRest] { let copiedReady = readyError.message; } }
}
fn unpublished() -> String {
    try { return "unpublished"; } finally { let badCleanup = CInt8(200); }
}
async fn main() {
    let ordinary = returning(); let completed = await ready(); let worker = await spawn returning();
    if ordinary != "retained" || completed != ordinary || worker != ordinary { panic("lostReturn"); }
    try { let ignored = unpublished(); } catch [cleanupError: Error, ...cleanupErrorRest] { let cleanupMessage = cleanupError.message; }
    var count = 0;
    while count < 3 { try { continue; } finally { count = count + 1; } }
    while true { try { break; } finally { count = count + 1; } }
    if count != 4 { panic("wrongLoopCleanup"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:?}"
    );
    assert_eq!(run.caught_errors.len(), 6);
    assert!(
        run.caught_errors
            .iter()
            .all(|failure| failure.primary.cause == FailureCause::Error(ErrorKind::Conversion))
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
}

#[test]
fn same_domain_future_preserves_producer_cleanup_and_repeated_observation() {
    let source = SourceFile::new(
        FileId(91),
        "future-errors.rinzu",
        r#"
async fn bad() -> CInt8 {
    try { return CInt8(200); }
    finally { try { let second = CInt8(300); } finally { let third = CInt8(400); } }
}
async fn main() {
    let future = bad(); let alias = future; var catches = 0;
    try { let ignored_one = await future; } catch one { catches = catches + 1; }
    try { let ignored_two = await alias; } catch two { catches = catches + 1; }
    if catches != 2 { panic("wrong_count"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none());
    assert_eq!(run.caught_errors.len(), 2);
    assert_eq!(run.caught_errors[0], run.caught_errors[1]);
    let failure = &run.caught_errors[0];
    assert_eq!(
        failure.primary.cause,
        FailureCause::Error(ErrorKind::Conversion)
    );
    assert_eq!(failure.cleanup.len(), 2);
    assert!(
        failure
            .cleanup
            .iter()
            .all(|diagnostic| diagnostic.cause == FailureCause::Error(ErrorKind::Conversion))
    );
    assert!(
        failure.primary.source.unwrap().start_byte < failure.cleanup[0].source.unwrap().start_byte
    );
    assert!(
        failure.cleanup[0].source.unwrap().start_byte
            < failure.cleanup[1].source.unwrap().start_byte
    );
    assert_eq!(run.transport.peak_transport_bytes, 0);
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(event, SourceAsyncEvent::WorkerCompleted { .. }))
            .count(),
        0
    );
    assert!(run.resources_returned);
}

#[test]
fn worker_handles_all_normal_errors_and_returns_a_transferable_report() {
    let source = SourceFile::new(
        FileId(92),
        "worker-catches.rinzu",
        r#"import rinzu.list;

fn bad() -> Int {
    try {
        try { let first = CInt8(200); }
        finally { try { let local = CInt8(300); } catch [caught: Error, ...caughtRest] { let message = caught.message; } let cleanup = CInt8(400); }
    } catch errors { return rinzu.list.length(errors); }
    return 0;
}
async fn main() {
    let future = spawn bad();
    let first = await future;
    let second = await future;
    if first != 2 || second != first { panic("wrongReport"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(run.caught_errors.len(), 2);
    assert!(run.caught_errors[0].cleanup.is_empty());
    assert_eq!(run.caught_errors[1].cleanup.len(), 1);
    // Private caught diagnostics also use transfer credit; no Error value
    // or unhandled normal failure crosses the Worker boundary.
    assert_eq!(run.transport.peak_transport_bytes, 94);
    assert!(run.resources_returned);
}

#[test]
fn rejected_worker_result_releases_partial_packets_and_preserves_origin_source() {
    let source = SourceFile::new(
        FileId(93),
        "worker-credit.rinzu",
        r#"
fn bad(value: Int) -> String { return "result exceeds transport credit"; }
async fn main() { let value = await spawn bad(200); }
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let admission =
        SpawnAdmissionController::from_target_profile(rinzu_runtime::spawn::SpawnLimits {
            worker_domains: 1,
            in_flight_per_origin: 1,
            process_transport_bytes: 18,
            operation_transport_bytes: 18,
        });
    let run = run_source_async_report_admitted(
        &artifact.ir,
        "main",
        SourceAsyncOptions::default(),
        admission,
    )
    .unwrap();
    let failure = run.failure_details.unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Overloaded);
    assert!(failure.primary.source.is_some());
    assert_eq!(run.transport.peak_transport_bytes, 8);
    assert!(run.resources_returned);
}
