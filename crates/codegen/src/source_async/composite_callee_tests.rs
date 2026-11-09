use super::*;
use crate::test_library::compile_library as compile_source;
use rinzu_compiler::{FileId, SourceFile};

#[test]
fn parked_prepared_and_completed_composite_callees_keep_their_owner_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "async-composite.rinzu",
        include_str!("../../tests/fixtures/async_composite.rinzu"),
    ))
    .unwrap();
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
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.peak_frames >= 4, "{run:?}");
    assert!(
        run.heap.collections > 100 && run.heap.moved_objects > 100,
        "{run:?}"
    );
    assert!(run.heap.peak_objects < 70, "{run:?}");
    assert_eq!(run.awaited_scalars.len(), 2);
}
#[test]
fn cancelled_composite_callees_keep_cleanup_roots_and_return_every_object() {
    let source = include_str!("../../tests/fixtures/async_composite.rinzu")
        .replace("async fn main()", "async fn successful_main()");
    let source = format!(
        "{source}\n{}",
        include_str!("../../tests/fixtures/async_composite_cancel.rinzu")
    );
    let artifact = compile_source(&SourceFile::new(
        FileId(1),
        "async-composite-cancel.rinzu",
        source,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            cancel_after_polls: Some(100),
            ..Default::default()
        },
    )
    .unwrap();
    let failure = run.failure_details.unwrap();
    assert_eq!(failure.primary.cause, FailureCause::Cancelled);
    assert_eq!(
        failure.cleanup.iter().map(|d| d.cause).collect::<Vec<_>>(),
        [FailureCause::Error(ErrorKind::Conversion); 2]
    );
    assert!(run.resources_returned);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.moved_objects > 0);
}
