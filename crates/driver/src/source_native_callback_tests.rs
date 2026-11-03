#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
mod fixture {
    include!("../tests/fixtures/native_callback_sdk/setup.rs");
}

#[test]
fn source_generic_callbacks_restore_actual_tokens_and_root_all_inputs_before_nested_body() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-source-generic-callback-{}",
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
    assert_eq!(run.native_calls.prepared, 79 * 9);
    assert_eq!(
        [
            run.native_calls.returned,
            run.native_calls.closed,
            run.native_calls.settled,
            run.native_calls.read,
            run.native_calls.released
        ],
        [79 * 9; 5]
    );
    assert_eq!(run.native_calls.abandoned, 0);
    assert_eq!(run.native_calls.callback_entries, 76 * 9);
    assert_eq!(run.native_calls.copied_bytes, 884 * 9);
    assert_eq!(run.caught_errors.len(), 4 * 9);
    assert!(run.caught_errors.iter().all(|failure| failure.primary.cause
        == rinzu_runtime::failure::FailureCause::Error(
            rinzu_runtime::failure::ErrorKind::Conversion
        )));
    assert_eq!(run.heap.userdata.created, 72);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.userdata.is_baseline());
    assert!(run.heap.moved_objects > 100);
    assert_eq!(run.worker_heaps.len(), 8);
    for worker in &run.worker_heaps {
        assert_eq!(worker.heap.userdata.created, 72);
        assert!(worker.heap.userdata.is_baseline());
        assert_eq!((worker.heap.objects, worker.heap.roots), (0, 0));
        assert!(worker.heap.moved_objects > 100);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_generic_callbacks_execute_pinned_bodies_on_main_and_workers_with_isolated_failures() {
    use rinzu_codegen::{
        BaselineFailure, BaselineOptions, BaselineRun, run_baseline_jit_with_package_bindings,
    };
    let root =
        std::env::temp_dir().join(format!("rinzu-jit-generic-callback-{}", std::process::id()));
    fixture::create(&root);
    std::fs::write(root.join("app/src/main.rinzu"),r#"
import callbacks;
fn execute()->Int {let value=callbacks.exercise();if value!=callbacks.original(){panic("lostValue");}return 76;}
fn failure()->Int {try {let invalid=CInt8(300);}finally {let cleaned=callbacks.exercise();}return 0;}
fn main() {let value=execute();if false {let failed=failure();}}
"#).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let options = BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let balanced = |run: &BaselineRun| {
        assert!(
            run.native_calls.is_baseline() && run.native_userdata.is_baseline(),
            "{run:?}"
        );
        assert_eq!(run.heap_objects, 0);
        let calls = &run.native_calls;
        assert_eq!(
            [
                calls.returned,
                calls.closed,
                calls.settled,
                calls.read,
                calls.released
            ],
            [calls.prepared; 5]
        );
        assert_eq!(calls.abandoned, 0);
        assert_eq!(run.published_code_versions, run.retired_code_versions);
        assert_eq!(run.rwx_mappings, 0);
    };
    let verify = |run: &BaselineRun| {
        balanced(run);
        assert_eq!(run.result, Ok(76), "{run:?}");
        assert_eq!(run.native_calls.prepared, 79);
        assert_eq!(run.native_calls.callback_entries, 76);
        assert_eq!(run.native_calls.copied_bytes, 884);
        assert_eq!(run.native_userdata.created, 72);
        assert_eq!(run.caught_errors.len(), 4);
        assert!(run.caught_errors.iter().all(|failure| failure.primary.cause
            == rinzu_runtime::failure::FailureCause::Error(
                rinzu_runtime::failure::ErrorKind::Conversion
            )));
        assert!(run.collection_count > 500);
    };
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    verify(&run);
    let module = std::sync::Arc::new(artifact.ir.clone());
    let artifacts = built
        .source_packages
        .iter()
        .map(|package| package.artifact.clone())
        .collect::<Vec<_>>();
    let workers = (0..8)
        .map(|_| {
            let module = std::sync::Arc::clone(&module);
            let artifacts = artifacts.clone();
            std::thread::spawn(move || {
                let packages =
                    rinzu_codegen::source_package::LoadedSourcePackages::load(&artifacts).unwrap();
                run_baseline_jit_with_package_bindings(
                    &module,
                    "execute",
                    options,
                    None,
                    Some(packages),
                )
                .unwrap()
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        verify(&worker.join().unwrap());
    }
    let failed = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "failure",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    balanced(&failed);
    assert_eq!(failed.result, Err(BaselineFailure::Conversion));
    assert_eq!(failed.native_calls.prepared, 79);
    assert_eq!(failed.native_userdata.created, 72);
    assert_eq!(failed.caught_errors.len(), 4);
    for limit in [1, 3, 5, 10, 20, 100, 1000, 5000] {
        let cancelled = run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            BaselineOptions {
                cancel_after_polls: Some(limit),
                ..options
            },
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        balanced(&cancelled);
        assert!(
            matches!(cancelled.result, Ok(76) | Err(BaselineFailure::Cancelled)),
            "{cancelled:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
