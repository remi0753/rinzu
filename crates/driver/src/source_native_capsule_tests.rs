#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
mod fixture {
    include!("../tests/fixtures/native_capsule_sdk/setup.rs");
}
#[path = "../tests/fixtures/native_capsule_sdk/admissions.rs"]
mod admissions;

#[test]
fn source_and_compiled_capsules_mask_cancellation_through_sync_callees_and_finally() {
    use rinzu_codegen::{BaselineFailure, BaselineOptions, run_baseline_jit_with_package_bindings};
    let root = std::env::temp_dir().join(format!(
        "rinzu-native-capsule-cancellation-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let source = include_str!("../tests/fixtures/native_capsule_sdk/cancel.rinzu");
    std::fs::write(root.join("app/src/main.rinzu"), source).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let source = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(built.load_source_packages().unwrap()),
            cancel_after_polls: Some(80),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        source.failure_details.unwrap().primary.cause,
        rinzu_runtime::failure::FailureCause::Cancelled
    );
    assert!(source.resources_returned && source.native_calls.is_baseline());
    assert_eq!(source.native_calls.prepared, 4);
    assert_eq!(source.native_calls.callback_entries, 1);
    let source = include_str!("../tests/fixtures/native_capsule_sdk/cancel.rinzu")
        .replace("async fn main()", "fn main()");
    std::fs::write(root.join("app/src/main.rinzu"), source).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let jit = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "main",
        BaselineOptions {
            collect_every_allocation: true,
            cancel_after_polls: Some(80),
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Err(BaselineFailure::Cancelled), "{jit:?}");
    assert!(jit.native_calls.is_baseline());
    assert_eq!(jit.native_calls.prepared, 4);
    assert_eq!(jit.native_calls.callback_entries, 1);
    assert_eq!(jit.heap_objects, 0);
    assert_eq!(jit.published_code_versions, jit.retired_code_versions);
    assert!(jit.collection_count > 10 && jit.rwx_mappings == 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_capsules_admit_callback_children_to_each_original_main_task_scope() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-source-native-capsules-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            package_root: manifest.root.clone(),
            source_packages: Some(built.load_source_packages().unwrap()),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(
        run.resources_returned && run.native_calls.is_baseline(),
        "{run:?}"
    );
    assert_eq!(
        [
            run.native_calls.prepared,
            run.native_calls.returned,
            run.native_calls.closed,
            run.native_calls.settled,
            run.native_calls.read,
            run.native_calls.released
        ],
        [13; 6]
    );
    assert_eq!(run.native_calls.callback_entries, 2);
    assert_eq!(run.native_calls.abandoned, 0);
    assert_eq!(run.worker_heaps.len(), 2);
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(
        run.heap.collections > 10 && run.heap.moved_objects > 10,
        "{run:?}"
    );
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(
                event,
                rinzu_codegen::SourceAsyncEvent::FileCompleted { bytes: 8193, .. }
            ))
            .count(),
        4
    );
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(
                event,
                rinzu_codegen::SourceAsyncEvent::WorkerCompleted { .. }
            ))
            .count(),
        2
    );
    admissions::check(&run.future_admissions);
    let compiled = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(compiled.exit_code, 0, "{}", compiled.output);
    assert!(compiled.retired && compiled.rwx_mappings == 0);
    admissions::check(&admissions::from_report(&compiled.output));
    assert!(compiled.output.contains("RINZU_FAILURE=none"));
    assert!(
        compiled
            .output
            .contains("RINZU_NATIVE_CALLS=13,13,13,13,13,13,2,0,0,0,0")
    );
    std::fs::remove_dir_all(root).unwrap();
}
