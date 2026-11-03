#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
use rinzu_runtime::failure::{ErrorKind, FailureCause};
mod fixture {
    include!("../tests/fixtures/native_data_sdk/setup.rs");
}

#[test]
fn source_native_data_receives_owned_results_and_distinct_errors_on_main_workers() {
    let root =
        std::env::temp_dir().join(format!("rinzu-source-native-data-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    let signature = artifact
        .ir
        .source_native_signatures
        .iter()
        .find(|signature| {
            signature.binding
                == rinzu_compiler::ast::NativeBinding::Symbol("buffer_fixture_cleanup".into())
        })
        .unwrap();
    let import = loaded
        .imports()
        .values()
        .find(|import| import.identity == signature.name)
        .unwrap();
    // SAFETY: validated ordinary Unit() binding; cleanup runs after all Workers.
    let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
    let normal = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(loaded),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(normal.failure_details.is_none(), "{normal:?}");
    assert!(normal.resources_returned && normal.native_calls.is_baseline());
    assert_eq!(
        [
            normal.native_calls.prepared,
            normal.native_calls.returned,
            normal.native_calls.closed,
            normal.native_calls.settled,
            normal.native_calls.read,
            normal.native_calls.released
        ],
        [166; 6]
    );
    assert_eq!(normal.native_calls.abandoned, 0);
    assert_eq!(normal.native_calls.callback_entries, 288);
    assert_eq!(normal.native_calls.copied_bytes, 1152);
    assert_eq!(normal.caught_errors.len(), 68);
    assert_eq!(
        normal
            .caught_errors
            .iter()
            .filter(
                |failure| failure.primary.cause == FailureCause::Error(ErrorKind::NullNativeValue)
            )
            .count(),
        11
    );
    assert_eq!(
        normal
            .caught_errors
            .iter()
            .filter(|failure| failure.primary.cause == FailureCause::Error(ErrorKind::InvalidUtf8))
            .count(),
        56
    );
    assert_eq!(
        normal.caught_errors.last().unwrap().primary.cause,
        FailureCause::Error(ErrorKind::Conversion)
    );
    // Each failing Worker catches once in its own Domain. Repeated await reads
    // its scalar report and does not duplicate or transfer the Error.
    assert_eq!(normal.worker_heaps.len(), 10);
    assert!(
        normal
            .worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    assert!(normal.heap.collections > 100);

    std::fs::write(app.join("src/main.rinzu"), "import buffers; async fn main() { let pending = spawn buffers.leaks(); let returned = await pending; }").unwrap();
    let (_, cancelled) = rinzu_compiler::check_package(&app).unwrap();
    let mut after_return = false;
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let run = run_source_async_report(
            &cancelled.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.resources_returned && run.native_calls.is_baseline());
        assert_eq!(run.native_calls.prepared, run.native_calls.released);
        after_return |= run.native_calls.returned > 0;
    }
    assert!(after_return);
    // SAFETY: every physical Worker has returned; no borrowed external data survives.
    unsafe { cleanup() };
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_jit_native_data_copies_results_and_releases_error_and_cancel_roots() {
    let root = std::env::temp_dir().join(format!("rinzu-jit-native-data-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    std::fs::write(app.join("src/main.rinzu"), r#"import buffers;
        import rinzu.core;
        fn execute() -> Int {
            let result = buffers.exercise(b"A\xff", "日本語\u{0}🙂");
            if result.text != "日本語" || result.bytes != b"\x00\xff\x07\x00" || result.caught != 7 { panic("wrong"); }
            return Int(utf8ByteLength(result.longText));
        }
        fn nullResult() -> String { return buffers.failure(0); }
        fn invalidResult() -> String { return buffers.failure(3); }
        fn leak() -> Int { return buffers.leaks(); }
        fn main() {
            try { if execute() != 65536 { panic("wrongLength"); } } catch unexpectedEntryErrors { panic("unexpectedEntryFailure"); }
            if false { let nullEntry = nullResult(); let invalidEntry = invalidResult(); let cancelEntry = leak(); }
        }
    "#).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let options = rinzu_codegen::BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    let verify = |run: &rinzu_codegen::BaselineRun| {
        assert_eq!(run.result, Ok(65536), "{run:?}");
        assert_eq!(run.caught_errors.len(), 7);
        assert_eq!(
            run.caught_errors[0].primary.cause,
            FailureCause::Error(ErrorKind::NullNativeValue)
        );
        assert!(run.caught_errors[1..].iter().all(|failure| failure.primary.cause == FailureCause::Error(ErrorKind::InvalidUtf8)));
        let calls = run.native_calls;
        assert_eq!(
            [
                calls.prepared,
                calls.returned,
                calls.closed,
                calls.settled,
                calls.read,
                calls.released
            ],
            [18; 6]
        );
        assert_eq!(calls.callback_entries, 32);
        assert_eq!(calls.copied_bytes, 128);
        assert!(calls.is_baseline() && calls.abandoned == 0);
        assert_eq!(run.heap_objects, 0);
        assert!(run.collection_count > 64);
        assert_eq!(run.published_code_versions, run.retired_code_versions);
        assert_eq!(run.rwx_mappings, 0);
    };
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
                rinzu_codegen::run_baseline_jit_with_package_bindings(
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
    for (entry, kind) in [
        ("nullResult", ErrorKind::NullNativeValue),
        ("invalidResult", ErrorKind::InvalidUtf8),
    ] {
        let failed = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            entry,
            options,
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        assert_eq!(
            failed.result,
            Err(rinzu_codegen::BaselineFailure::Error(kind))
        );
        assert_eq!(
            failed.failure_details.as_ref().unwrap().primary.cause,
            FailureCause::Error(kind)
        );
        assert!(failed.native_calls.is_baseline());
        assert_eq!(failed.native_calls.prepared, 1);
        assert_eq!(failed.native_calls.released, 1);
        assert_eq!(failed.heap_objects, 0);
    }
    let mut received_before_cancel = false;
    for limit in [1, 3, 5, 10, 20, 100, run.poll_count] {
        let cancelled = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "leak",
            rinzu_codegen::BaselineOptions {
                cancel_after_polls: Some(limit),
                ..options
            },
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        assert!(cancelled.native_calls.is_baseline());
        assert_eq!(
            cancelled.native_calls.prepared,
            cancelled.native_calls.released
        );
        assert_eq!(cancelled.heap_objects, 0);
        received_before_cancel |= cancelled.native_calls.returned > 0;
    }
    assert!(received_before_cancel);
    let loaded = built.load_source_packages().unwrap();
    let signature = artifact
        .ir
        .source_native_signatures
        .iter()
        .find(|signature| {
            signature.binding
                == rinzu_compiler::ast::NativeBinding::Symbol("buffer_fixture_cleanup".into())
        })
        .unwrap();
    let import = loaded
        .imports()
        .values()
        .find(|import| import.identity == signature.name)
        .unwrap();
    // SAFETY: the validated Unit() function only releases the fixture's private
    // protected page after every physical test thread and managed heap finished.
    let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
    // SAFETY: no later call or borrowed external storage accesses that page.
    unsafe { cleanup() };
    std::fs::remove_dir_all(root).unwrap();
}
