use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};

#[test]
fn typed_function_references_execute_on_main_workers_and_pending_callees() {
    let source = format!(
        "{}\nfn safeVerify()->Int {{try{{return verify();}}catch errors{{return 0;}}}} async fn forwarded() -> Int {{ return await spawn safeVerify(); }} async fn main() {{ let on_main=verify(); let reference=forwarded; let pending=reference(); let on_worker=await pending; if on_main!=42 || on_worker!=42 {{ panic(\"bad_result\"); }} }}",
        include_str!("../../tests/fixtures/function_reference.rinzu")
            .replace("fn main()", "fn unused_main()")
    );
    let artifact =
        compile_source(&SourceFile::new(FileId(1), "reference-owner.rinzu", source)).unwrap();
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
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:?}"
    );
    assert_eq!(run.caught_errors.len(), 4);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert_eq!(run.worker_heaps.len(), 1);
    assert_eq!(
        (
            run.worker_heaps[0].heap.objects,
            run.worker_heaps[0].heap.roots
        ),
        (0, 0)
    );
    assert!(run.worker_heaps[0].heap.collections > 50 && run.worker_heaps[0].heap.peak_frames >= 3);
}

#[test]
fn main_and_sync_frames_execute_composites_bytes_and_gc_on_one_owner_heap() {
    for source in [
        include_str!("../../tests/fixtures/composite.rinzu"),
        include_str!("../../tests/fixtures/bytes_collection.rinzu"),
    ] {
        let source = source.replace("fn main()", "async fn main()");
        let artifact =
            compile_source(&SourceFile::new(FileId(1), "owner-heap.rinzu", source)).unwrap();
        let run = run_source_async(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.result, 0);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert!(run.resources_returned);
        assert!(run.heap.collections > 50, "{run:?}");
        assert!(run.heap.moved_objects > 100, "{run:?}");
        assert!(run.heap.peak_frames >= 3, "{run:?}");
        assert!(run.heap.peak_objects < 100, "{run:?}");
    }
}
