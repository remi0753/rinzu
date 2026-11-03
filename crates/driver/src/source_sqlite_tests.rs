#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod fixture {
    include!("../tests/fixtures/sqlite_native_data/setup.rs");
}

#[test]
fn sqlite_classes_and_context_callbacks_preserve_fallbacks_and_owner_lifetimes() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-sqlite-native-observer-{}",
        std::process::id()
    ));
    fixture::create_observer(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(52), "{jit:?}");
    assert!(jit.native_calls.is_baseline() && jit.native_userdata.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    let run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned && run.native_calls.is_baseline(),
        "{run:?}"
    );
    assert_eq!(run.stdout, b"52\n");
    assert!(run.heap.userdata.is_baseline());
    assert_eq!(run.native_process.userdata, Default::default());
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sqlite_native_record_results_write_back_success_and_business_errors_on_owners() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-sqlite-native-results-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(53), "{jit:?}");
    assert!(jit.native_calls.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    let run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned && run.native_calls.is_baseline(),
        "{run:?}"
    );
    assert_eq!(run.stdout, b"53\n53\n");
    assert_eq!(run.caught_errors.len(), 8);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(!run.worker_heaps.is_empty());
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    std::fs::remove_dir_all(root).unwrap();
}
