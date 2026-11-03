#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn adaptive_and_stress_preserve_alias_cycles_suspension_and_worker_snapshots() {
    let text = include_str!("fixtures/m2_state.rinzu").replace("i < 300", "i < 5000");
    let artifact = compile_source(&SourceFile::new(FileId(1), "paced-async.rinzu", &text)).unwrap();
    let normal = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    let stress = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(normal.failure_details.is_none() && stress.failure_details.is_none());
    assert_eq!(normal.stdout, stress.stdout);
    assert_eq!(
        normal.stdout,
        b"selection=5,undo=1\nafter\ncurrent=updated-after-snapshot\n"
    );
    assert!(
        normal.heap.collections > 1,
        "pressure must trigger GC during execution"
    );
    assert!(normal.heap.collections * 20 < stress.heap.collections);
    for run in [&normal, &stress] {
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert!(run.resources_returned);
        assert!(
            run.worker_heaps
                .iter()
                .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
        );
    }
    assert!(stress.heap.moved_objects > 300);
}

#[test]
fn baseline_jit_uses_the_same_pacing_and_keeps_its_moving_gc_stress_option() {
    let text = include_str!("fixtures/m2_state.rinzu")
        .replace("i < 300", "i < 5000")
        .replace("async fn main()", "fn main()")
        .replace("spawn snapshot(", "snapshot(")
        .replace("await pending", "pending");
    let artifact = compile_source(&SourceFile::new(FileId(2), "paced-sync.rinzu", text)).unwrap();
    let normal = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    let stress = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(normal.result, Ok(0));
    assert_eq!(normal.result, stress.result);
    assert_eq!(normal.stdout, stress.stdout);
    assert!(normal.collection_count > 1);
    assert!(normal.collection_count * 20 < stress.collection_count);
    assert_eq!(normal.heap_objects, 0);
    assert_eq!(stress.heap_objects, 0);
    assert!(stress.collection_count >= stress.allocation_count);
}
