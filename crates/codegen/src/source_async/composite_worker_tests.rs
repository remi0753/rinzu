use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};
use rinzu_runtime::transport::TransportDirection;

#[test]
fn rejected_composite_results_and_partial_diagnostics_release_the_real_worker_heap() {
    let functions = r#"
class Packet { const text: String, const bytes: Bytes(4), const high: CSize }
fn expanded(value: Packet) -> List<Packet> { return [value,value,value,value]; }
fn observed(value: Packet) -> Packet {
    try { let first = CInt8(200); } catch [one: Error, ...oneRest] { let first_message = one.message; }
    try { let second = CInt8(300); } catch [two: Error, ...twoRest] { let second_message = two.message; }
    return value;
}
"#;
    for (target, process_limit, operation_limit) in [("expanded", 128, 128), ("observed", 100, 128)]
    {
        let source = format!(
            "{functions}\nasync fn main() {{ let original = Packet(text: \"日本語\\u{{0}}🙂\",bytes:b\"A\\xff\",high:18446744073709551615); let waiting = spawn {target}(original); let value = await waiting; }}"
        );
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "worker-composite-budget.rinzu",
            source.clone(),
        ))
        .unwrap();
        let admission =
            SpawnAdmissionController::from_target_profile(rinzu_runtime::spawn::SpawnLimits {
                worker_domains: 1,
                in_flight_per_origin: 1,
                process_transport_bytes: process_limit,
                operation_transport_bytes: operation_limit,
            });
        let run = run_source_async_report_admitted(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                record_transport_evidence: true,
                ..Default::default()
            },
            admission,
        )
        .unwrap();
        let failure = run.failure_details.as_ref().unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Overloaded);
        assert_eq!(
            failure.primary.source.unwrap().start_byte as usize,
            source.find("spawn ").unwrap()
        );
        assert!(run.resources_returned && run.caught_errors.is_empty());
        assert_eq!(run.worker_heaps.len(), 1);
        assert_eq!(
            (
                run.worker_heaps[0].heap.objects,
                run.worker_heaps[0].heap.roots
            ),
            (0, 0)
        );
        assert!(
            run.worker_heaps[0].heap.peak_objects >= 4 && run.worker_heaps[0].heap.collections > 0
        );
        assert!(
            run.transport_evidence
                .iter()
                .all(|credit| credit.released_at.is_some())
        );
        if target == "expanded" {
            assert!(
                run.transport_evidence
                    .iter()
                    .all(|credit| credit.direction == TransportDirection::Input)
            );
        } else {
            assert!(
                run.transport_evidence
                    .iter()
                    .filter(|credit| credit.direction == TransportDirection::Result)
                    .count()
                    >= 2
            );
        }
    }
}

#[test]
fn rejected_ninth_composite_spawn_never_starts_and_dropped_admitted_children_all_join() {
    let source = r#"
class Packet { const text: String, const bytes: Bytes(4), const high: CSize }
fn echo(value: Packet) -> Packet { return value; }
async fn main() {
    let original=Packet(text:"retained",bytes:b"A\xff",high:18446744073709551615);
    let one=spawn echo(original); let two=spawn echo(original); let three=spawn echo(original); let four=spawn echo(original);
    let five=spawn echo(original); let six=spawn echo(original); let seven=spawn echo(original); let eight=spawn echo(original);
    let denied=spawn echo(original);
    try { let unexpected=await denied; } catch [error: Error, ...errorRest] { let message=error.message; }
}
"#;
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "worker-composite-admission.rinzu",
        source,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.future_admissions.len(), 8);
    assert!(run.resources_returned, "{run:?}");
    assert_eq!(
        run.failure_details.as_ref().unwrap().primary.cause,
        FailureCause::Overloaded
    );
    // Scope failure can cancel queued Workers before their heap/body entry.
    // All eight accepted operations must still deliver physical completion.
    assert_eq!(run.worker_returns, 8);
    assert!(run.worker_heaps.len() <= 8);
    assert!(run.caught_errors.is_empty());
    assert_eq!(run.transport.peak_worker_domains, 8);
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    assert!(
        run.transport_evidence
            .iter()
            .all(|credit| credit.operation != Some(8) && credit.released_at.is_some())
    );
}

#[test]
fn source_workers_prepare_all_composites_and_return_copies_before_origin_settlement() {
    let source = include_str!("../../tests/fixtures/worker_composite.rinzu");
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "worker-composite.rinzu",
        source,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(run.resources_returned);
    assert_eq!(run.worker_heaps.len(), 9);
    assert!(run.worker_heaps.iter().all(|worker| worker.domain != 0
        && worker.heap.objects == 0
        && worker.heap.roots == 0
        && worker.heap.collections > 50
        && worker.heap.moved_objects > 50
        && worker.heap.peak_frames >= 2));
    assert!(run.heap.collections > 100 && run.heap.moved_objects > 100);
    assert_eq!(run.caught_errors.len(), 1);
    assert_eq!(
        run.caught_errors[0].primary.source.unwrap().start_byte as usize,
        source.find("CInt8(200)").unwrap()
    );
    assert_eq!(
        run.caught_errors[0].cleanup[0].source.unwrap().start_byte as usize,
        source.find("CInt8(300)").unwrap()
    );
    for input in run
        .transport_evidence
        .iter()
        .filter(|credit| credit.direction == TransportDirection::Input)
    {
        let released = input.released_at.unwrap();
        for result in run.transport_evidence.iter().filter(|credit| {
            credit.operation == input.operation && credit.direction == TransportDirection::Result
        }) {
            assert!(released < result.reserved_at && result.released_at.is_some());
        }
    }
    assert!(
        run.events
            .iter()
            .filter_map(|event| {
                if let SourceAsyncEvent::WorkerCompleted { worker_thread, .. } = event {
                    Some(worker_thread)
                } else {
                    None
                }
            })
            .all(|thread| *thread != run.owner_thread)
    );
}

#[test]
fn cancelled_worker_composites_handle_local_cleanup_and_reclaim_the_actual_heap() {
    let source = include_str!("../../tests/fixtures/worker_composite.rinzu")
        .replace("async fn main()", "async fn successful_main()");
    let source = format!(
        "{source}\n{}",
        include_str!("../../tests/fixtures/worker_composite_cancel.rinzu")
    );
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "worker-composite-cancel.rinzu",
        source.clone(),
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            cancel_worker_after_polls: Some(100),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    let failure = run.failure_details.as_ref().unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Cancelled);
    assert_eq!(
        failure.cleanup.iter().map(|d| d.cause).collect::<Vec<_>>(),
        [FailureCause::Error(ErrorKind::Conversion)]
    );
    assert_eq!(
        failure
            .cleanup
            .iter()
            .map(|d| d.source.unwrap().start_byte as usize)
            .collect::<Vec<_>>(),
        [source.find("CInt8(401)").unwrap()]
    );
    assert_eq!(run.caught_errors.len(), 1);
    assert_eq!(
        run.caught_errors[0].primary.source.unwrap().start_byte as usize,
        source.find("CInt8(301)").unwrap()
    );
    assert!(run.resources_returned && run.heap.objects == 0 && run.heap.roots == 0);
    assert_eq!(run.worker_heaps.len(), 1);
    let heap = run.worker_heaps[0].heap;
    assert!(heap.collections > 5 && heap.moved_objects > 5 && heap.objects == 0 && heap.roots == 0);
    assert!(
        run.transport_evidence
            .iter()
            .all(|credit| credit.released_at.is_some())
    );
}
