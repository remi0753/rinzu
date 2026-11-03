#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod fixture {
    include!("../tests/fixtures/native_byte_array_sdk/setup.rs");
}

#[test]
fn byte_array_results_match_source_jit_aot_workers_and_external_lifetimes() {
    let root = std::env::temp_dir().join(format!("rinzu-native-byte-array-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let run = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        run.failure_details.is_none() && run.resources_returned,
        "{run:?}"
    );
    assert!(run.native_calls.is_baseline());
    assert_eq!(run.native_calls.prepared, run.native_calls.released);
    assert_eq!(run.worker_heaps.len(), 2);
    assert!(
        run.worker_heaps
            .iter()
            .all(|w| w.heap.objects == 0 && w.heap.roots == 0)
    );
    let entry = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name.ends_with("::exercise"))
        .unwrap()
        .name
        .clone();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        &entry,
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(65540), "{jit:?}");
    assert!(jit.native_calls.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    assert_eq!(
        crate::aot::verify_equivalence(&app, None).unwrap().outcome,
        "success"
    );
    std::fs::remove_dir_all(root).unwrap();
}
