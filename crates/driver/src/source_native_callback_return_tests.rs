#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod fixture {
    include!("../tests/fixtures/native_callback_return_sdk/setup.rs");
}
mod lifetime {
    include!("../tests/fixtures/native_callback_return_sdk/lifetime.rs");
}
mod userdata_fixture {
    include!("../tests/fixtures/native_userdata_sdk/setup.rs");
}

#[test]
fn process_resource_snapshots_separate_other_connections_closed_growth_and_return_buffers() {
    use rinzu_codegen::{NativeProcessEvidence, SourceAsyncOptions, run_source_async_report};
    const CHILD: &str = "RINZU_RESOURCE_SNAPSHOT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_native_callback_return_tests::process_resource_snapshots_separate_other_connections_closed_growth_and_return_buffers", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    let root = std::env::temp_dir().join(format!("rinzu-resource-snapshot-{}", std::process::id()));
    let returns_root = root.join("returns");
    fixture::create(&returns_root);
    std::fs::write(returns_root.join("app/src/main.rinzu"), "import returns;async fn main(){var index=0;while index<60 {let value=returns.exercise();if value!=56 {panic(\"lost\");}index=index+1;}}").unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&returns_root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    unsafe extern "C" fn unused_handler(
        _: *mut core::ffi::c_void,
        _: *const rinzu_runtime::callback_entry::CallbackFrame,
    ) -> u64 {
        0
    }
    let monitor = rinzu_runtime::native_diagnostics::ActivityMonitor::new();
    let mut context = 0_u64;
    // SAFETY: this independent connection is never invoked. Its context and
    // diagnostic monitor remain live until close on this same physical thread.
    let other = unsafe {
        let callback = rinzu_runtime::callback_entry::FrameCallback::prepare_deferred(
            unused_handler,
            std::ptr::from_mut(&mut context).cast(),
        );
        callback.bind_diagnostics(
            &monitor,
            c"other.connection",
            rinzu_runtime::failure::SourcePosition {
                file: 17,
                start_byte: 31,
                end_byte: 49,
            },
            1,
            1,
        );
        callback
    };
    let source = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(source.resources_returned && source.native_calls.is_baseline());
    let pool = source.native_process.callback_pool;
    assert_eq!(
        (pool.entries, pool.closed_entries, pool.blocks),
        (781, 780, 2)
    );
    assert_eq!(
        (pool.live_connections, pool.in_flight, pool.live_diagnostics),
        (1, 0, 1)
    );
    assert!(pool.diagnostic_bytes > 0 && pool.tombstone_bytes > 0 && pool.block_metadata_bytes > 0);
    assert_eq!(
        source.native_process.callback_returns,
        source.native_calls.callback_returns
    );
    assert_eq!(source.native_process.callback_returns.bytes, 15960);
    assert!(other.close());
    let closed = NativeProcessEvidence::capture();
    assert_eq!(closed.callback_pool.closed_entries, 781);
    assert_eq!(
        (
            closed.callback_pool.live_connections,
            closed.callback_pool.live_diagnostics,
            closed.callback_pool.diagnostic_bytes
        ),
        (0, 0, 0)
    );
    assert!(closed.callback_pool.tombstone_bytes > pool.tombstone_bytes);
    assert_eq!(
        closed.callback_returns,
        source.native_process.callback_returns
    );
    drop(other);
    drop(monitor);

    std::fs::write(returns_root.join("app/src/main.rinzu"), "import returns;fn execute()->Int {var index=0;while index<60 {let value=returns.exercise();if value!=56 {panic(\"lost\");}index=index+1;}return 56;}fn main(){let result=execute();}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&returns_root.join("app")).unwrap();
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
    assert_eq!(jit.result, Ok(56));
    assert!(jit.native_calls.is_baseline() && jit.heap_objects == 0);
    assert_eq!(
        jit.native_calls.callback_returns,
        source.native_calls.callback_returns
    );
    let closed = jit.native_process;
    assert_eq!(
        (
            closed.callback_pool.entries,
            closed.callback_pool.closed_entries,
            closed.callback_pool.blocks
        ),
        (1561, 1561, 4)
    );
    assert_eq!(
        (
            closed.callback_pool.live_connections,
            closed.callback_pool.live_diagnostics,
            closed.callback_pool.diagnostic_bytes
        ),
        (0, 0, 0)
    );
    assert_eq!(closed.callback_returns.bytes, 31920);

    // A second actual SDK uses Main and physical Worker context scopes. Their
    // completion leaves no token metadata after their heaps and roots end.
    let userdata_root = root.join("userdata");
    userdata_fixture::create(&userdata_root);
    let (manifest, artifact) = rinzu_compiler::check_package(&userdata_root.join("app")).unwrap();
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
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(source.resources_returned && source.heap.userdata.is_baseline());
    let tokens = source.heap.userdata.created
        + source
            .worker_heaps
            .iter()
            .map(|worker| {
                assert!(worker.heap.userdata.is_baseline());
                worker.heap.userdata.created
            })
            .sum::<usize>();
    let process = source.native_process;
    assert!(tokens > 0);
    assert_eq!(process.userdata, Default::default());
    assert_eq!(
        process.callback_pool.entries,
        process.callback_pool.closed_entries
    );
    assert_eq!(
        (
            process.callback_pool.live_connections,
            process.callback_pool.in_flight,
            process.callback_pool.live_diagnostics,
            process.callback_pool.diagnostic_bytes
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(process.callback_returns, closed.callback_returns);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_and_jit_callback_buffers_prepare_fallbacks_and_copy_results_without_retaining_heap_values()
 {
    use rinzu_codegen::{
        BaselineOptions, CallbackReturnEvidence, SourceAsyncOptions,
        run_baseline_jit_with_package_bindings, run_source_async_report,
    };
    let root = std::env::temp_dir().join(format!(
        "rinzu-source-jit-callback-return-{}",
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
    let expected = CallbackReturnEvidence {
        normal: 24,
        fallback: 12,
        bytes: 266,
        metadata_bytes: 864,
    };
    let source = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        source.failure_details.is_none()
            && source.resources_returned
            && source.native_calls.is_baseline(),
        "{source:?}"
    );
    assert_eq!(source.native_calls.prepared, 70 * 9 + 1);
    assert_eq!(source.native_calls.callback_entries, 56 * 9);
    assert_eq!(source.native_calls.copied_bytes, 160 * 9);
    assert_eq!(
        source.native_calls.callback_returns,
        CallbackReturnEvidence {
            normal: expected.normal * 9,
            fallback: expected.fallback * 9,
            bytes: expected.bytes * 9,
            metadata_bytes: expected.metadata_bytes * 9
        }
    );
    assert_eq!(source.caught_errors.len(), 8 * 9);
    assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
    assert!(
        source
            .worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    let api = root.join("app/src/main.rinzu");
    std::fs::write(api,"import returns;fn execute()->Int {return returns.exercise();}fn main(){let value=execute();}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let options = BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let verify = |run: rinzu_codegen::BaselineRun| {
        assert_eq!(run.result, Ok(56), "{run:?}");
        assert!(run.native_calls.is_baseline());
        assert_eq!(run.heap_objects, 0);
        assert_eq!(run.native_calls.prepared, 70);
        assert_eq!(run.native_calls.callback_entries, 56);
        assert_eq!(run.native_calls.copied_bytes, 160);
        assert_eq!(run.native_calls.callback_returns, expected);
        assert_eq!(run.caught_errors.len(), 8);
        assert_eq!(run.published_code_versions, run.retired_code_versions);
    };
    verify(
        run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            options,
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap(),
    );
    let module = std::sync::Arc::new(artifact.ir);
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
        verify(worker.join().unwrap());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn callback_buffers_remain_readable_after_source_and_jit_owners_and_worker_heaps_end() {
    use rinzu_codegen::{
        BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_package_bindings,
        run_source_async_report,
    };
    let root = std::env::temp_dir().join(format!(
        "rinzu-callback-buffer-lifetime-{}",
        std::process::id()
    ));
    fixture::create(&root);
    lifetime::retain_until_shutdown(&root);
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
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        source.failure_details.is_none()
            && source.resources_returned
            && source.native_calls.is_baseline()
    );
    assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
    assert!(
        source
            .worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    let api = root.join("app/src/main.rinzu");
    std::fs::write(&api,"import returns;fn execute()->Int {return returns.exercise();}fn main(){let value=execute();}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let options = BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(run.result, Ok(56));
    assert_eq!(run.heap_objects, 0);
    assert!(run.native_calls.is_baseline());
    assert_eq!(run.published_code_versions, run.retired_code_versions);
    let module = std::sync::Arc::new(artifact.ir);
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
        let run = worker.join().unwrap();
        assert_eq!(run.result, Ok(56));
        assert_eq!(run.heap_objects, 0);
        assert!(run.native_calls.is_baseline());
        assert_eq!(run.published_code_versions, run.retired_code_versions);
    }
    // A fresh Owner validates native pointers after every producing Owner ended.
    std::fs::write(&api,"import returns;fn main(){try{var index=0;while index<1000 {let pressure=[\"pressure\",\"new moving collection\"];if pressure!=[\"pressure\",\"new moving collection\"]{panic(\"lost\");}index=index+1;}if returns.verifyHeld()!=1 {panic(\"invalid\");}}catch errors{panic(\"unexpectedFailure\");}}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let verify = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "main",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(verify.result, Ok(0));
    assert_eq!(verify.heap_objects, 0);
    assert!(verify.native_calls.is_baseline());
    assert_eq!(
        verify.native_calls.callback_returns,
        rinzu_codegen::CallbackReturnEvidence::default()
    );
    assert_eq!(verify.published_code_versions, verify.retired_code_versions);
    std::fs::remove_dir_all(root).unwrap();
}
