#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
mod fixture {
    include!("../tests/fixtures/native_supplant_sdk/setup.rs");
}

#[test]
fn actual_worker_shutdown_blocker_retains_roots_until_native_return_and_origin_receipt() {
    const CHILD_ROOT: &str = "RINZU_WORKER_BLOCKER_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        let shutdown = std::env::var("RINZU_WORKER_BLOCKER_MODE").unwrap() == "shutdown";
        let (cancel, shutdown_limit) = if shutdown {
            (None, Some(80))
        } else {
            (Some(80), None)
        };
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
                package_root: manifest.root.clone(),
                cancel_after_polls: cancel,
                shutdown_after_polls: shutdown_limit,
                // Many actual wait expirations must retain the still-running Worker.
                completion_timeout: std::time::Duration::from_millis(25),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            source.failure_details.as_ref().unwrap().primary.cause,
            if shutdown {
                rinzu_runtime::failure::FailureCause::Shutdown
            } else {
                rinzu_runtime::failure::FailureCause::Cancelled
            }
        );
        assert!(source.resources_returned && source.native_calls.is_baseline());
        assert_eq!(source.native_calls.returned, 5);
        assert_eq!(source.native_calls.callback_entries, 0);
        assert_eq!(source.worker_returns, 1);
        assert_eq!(source.worker_heaps.len(), 1);
        let heap = source.worker_heaps[0].heap;
        assert_eq!((heap.objects, heap.roots), (0, 0));
        assert!(heap.collections > 50 && heap.peak_frames >= 2);
        let jit =
            crate::compiled_async::run(&manifest, &artifact, cancel, shutdown_limit, true).unwrap();
        assert_eq!(jit.exit_code, 1);
        assert!(jit.retired && jit.rwx_mappings == 0);
        crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
        crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
        println!("WORKER_BLOCKER_RESOURCE_BASELINE");
        return;
    }
    for mode in ["cancel", "shutdown"] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-worker-blocker-{mode}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        std::fs::write(
            root.join("app/src/main.rinzu"),
            include_str!("../tests/fixtures/native_supplant_sdk/worker-blocker.rinzu"),
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_native_supplant_tests::actual_worker_shutdown_blocker_retains_roots_until_native_return_and_origin_receipt", "--nocapture"])
            .env(CHILD_ROOT, &root).env("RINZU_WORKER_BLOCKER_MODE", mode).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("WORKER_BLOCKER_RESOURCE_BASELINE")
        );
        let warnings = String::from_utf8_lossy(&output.stderr);
        let blockers = warnings
            .lines()
            .filter(|line| line.contains("shutdown-blocker symbol=supplant_worker_block "))
            .collect::<Vec<_>>();
        assert_eq!(blockers.len(), 2, "{warnings}");
        for warning in blockers {
            assert!(
                !warning.contains("source=0:")
                    && warning.contains("owner=0x")
                    && warning.contains("stack=supplant_worker_block"),
                "{warning}"
            );
        }
        assert_eq!(
            warnings.matches("WORKER_BLOCKER_NATIVE_ENTER").count(),
            2,
            "{warnings}"
        );
        assert_eq!(
            warnings.matches("WORKER_BLOCKER_NATIVE_RETURN").count(),
            2,
            "{warnings}"
        );
        assert_eq!(
            warnings.matches("WORKER_BLOCKER_WORKER_FINALLY").count(),
            2,
            "{warnings}"
        );
        assert!(!warnings.contains("runtime error:"), "{warnings}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn actual_shutdown_blocker_preserves_native_roots_and_settles_after_real_return() {
    const CHILD_ROOT: &str = "RINZU_SHUTDOWN_BLOCKER_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
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
                package_root: manifest.root.clone(),
                shutdown_after_polls: Some(80),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            source.failure_details.as_ref().unwrap().primary.cause,
            rinzu_runtime::failure::FailureCause::Shutdown
        );
        assert!(source.resources_returned && source.native_calls.is_baseline());
        assert_eq!(source.native_calls.returned, 4);
        assert_eq!(source.native_calls.callback_entries, 1);
        assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
        let jit = crate::compiled_async::run(&manifest, &artifact, None, Some(80), true).unwrap();
        assert_eq!(jit.exit_code, 1);
        assert!(jit.retired && jit.rwx_mappings == 0);
        crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
        crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
        println!("SHUTDOWN_BLOCKER_RESOURCE_BASELINE");
        return;
    }
    let root = std::env::temp_dir().join(format!("rinzu-shutdown-blocker-{}", std::process::id()));
    fixture::create(&root);
    std::fs::write(
        root.join("app/src/main.rinzu"),
        include_str!("../tests/fixtures/native_supplant_sdk/blocker.rinzu"),
    )
    .unwrap();
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","source_native_supplant_tests::actual_shutdown_blocker_preserves_native_roots_and_settles_after_real_return","--nocapture"]).env(CHILD_ROOT,&root).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("SHUTDOWN_BLOCKER_RESOURCE_BASELINE"));
    let warnings = String::from_utf8_lossy(&output.stderr);
    let blockers = warnings
        .lines()
        .filter(|line| line.contains("shutdown-blocker symbol=supplant_check_run "))
        .collect::<Vec<_>>();
    assert_eq!(blockers.len(), 2, "{warnings}");
    for warning in blockers {
        assert!(
            !warning.contains("source=0:")
                && warning.contains("owner=0x")
                && warning.contains("stack=supplant_check_run"),
            "{warning}"
        );
    }
    assert!(!warnings.contains("runtime error:"), "{warnings}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "private subprocess witness for fatal allocation injection"]
fn supplant_preparation_allocation_probe() {
    let root = std::path::PathBuf::from(std::env::var_os("RINZU_PROBE_ROOT").unwrap());
    let budget: usize = std::env::var("RINZU_PROBE_BUDGET")
        .unwrap()
        .parse()
        .unwrap();
    let engine = std::env::var("RINZU_PROBE_ENGINE").unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    if engine == "source" {
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let packages = built.load_source_packages().unwrap();
        rinzu_codegen::allocation_probe::configure(budget);
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(packages),
                package_root: manifest.root,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            run.failure_details.is_none()
                && run.resources_returned
                && run.native_calls.is_baseline()
        );
        println!(
            "SUPPLANT_PREPARE_ALLOCATIONS={}",
            rinzu_codegen::allocation_probe::allocations().unwrap()
        );
    } else {
        assert_eq!(engine, "jit");
        crate::compiled_async::configure_preparation_probe(budget);
        let run = crate::compiled_async::run(&manifest, &artifact, None, None, false).unwrap();
        assert_eq!(run.exit_code, 0, "{}", run.output);
        assert!(run.retired);
        print!("{}", run.output);
    }
}

#[test]
fn source_and_compiled_supplant_actual_prepare_allocations_fail_without_entry() {
    for engine in ["source", "jit"] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-supplant-probe-{engine}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        std::fs::write(
            root.join("app/src/main.rinzu"),
            include_str!("../tests/fixtures/native_supplant_sdk/result-restart.rinzu"),
        )
        .unwrap();
        let run = |budget: usize| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "source_native_supplant_tests::supplant_preparation_allocation_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .env("RINZU_PROBE_ROOT", &root)
                .env("RINZU_PROBE_ENGINE", engine)
                .env("RINZU_PROBE_BUDGET", budget.to_string())
                .output()
                .unwrap()
        };
        let complete = run(10000);
        assert!(complete.status.success(), "{engine}: {complete:?}");
        let stdout = String::from_utf8_lossy(&complete.stdout);
        let count: usize = stdout
            .lines()
            .find_map(|line| line.strip_prefix("SUPPLANT_PREPARE_ALLOCATIONS="))
            .unwrap()
            .parse()
            .unwrap();
        assert!(count > 0 && count < 10000, "{stdout}");
        for budget in 0..count {
            let output = run(budget);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                matches!(output.status.code(), Some(70 | 71)),
                "{engine} budget={budget}: {stdout}\n{stderr}"
            );
            assert!(
                !stderr.contains("SUPPLANT_NATIVE_ENTER")
                    && !stderr.contains("SUPPLANT_CALLBACK_FINALLY")
                    && !stderr.contains("SUPPLANT_OUTER_FINALLY")
                    && !stdout.contains("RINZU_RESULT="),
                "{engine} budget={budget}: {stdout}\n{stderr}"
            );
        }
        eprintln!("verified {count} actual {engine} supplant preparation allocations");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn source_and_compiled_supplants_cancel_before_entry_and_discard_running_results() {
    use rinzu_runtime::failure::FailureCause;
    for (name, source, cancel, shutdown, cause, abandoned, callbacks) in [
        (
            "queued",
            include_str!("../tests/fixtures/native_supplant_sdk/queued-cancel.rinzu"),
            None,
            None,
            Some(FailureCause::Cancelled),
            1,
            0,
        ),
        (
            "running",
            include_str!("../tests/fixtures/native_supplant_sdk/cancel.rinzu"),
            Some(80),
            None,
            Some(FailureCause::Cancelled),
            0,
            1,
        ),
        (
            "restart",
            include_str!("../tests/fixtures/native_supplant_sdk/result-restart.rinzu"),
            None,
            None,
            None,
            0,
            2,
        ),
        (
            "queued-shutdown",
            include_str!("../tests/fixtures/native_supplant_sdk/queued-cancel.rinzu"),
            None,
            Some(40),
            Some(FailureCause::Shutdown),
            1,
            0,
        ),
        (
            "running-shutdown",
            include_str!("../tests/fixtures/native_supplant_sdk/cancel.rinzu"),
            None,
            Some(80),
            Some(FailureCause::Shutdown),
            0,
            1,
        ),
        (
            "child-shutdown",
            include_str!("../tests/fixtures/native_supplant_sdk/shutdown-child.rinzu"),
            None,
            Some(80),
            Some(FailureCause::Shutdown),
            0,
            1,
        ),
    ] {
        eprintln!("supplant gate {name}: Source and compiled JIT");
        let root = std::env::temp_dir().join(format!(
            "rinzu-supplant-gates-{name}-{}",
            std::process::id()
        ));
        fixture::create(&root);
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
                package_root: manifest.root.clone(),
                cancel_after_polls: cancel,
                shutdown_after_polls: shutdown,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            source
                .failure_details
                .as_ref()
                .map(|failure| failure.primary.cause),
            cause,
            "{name}: {source:?}"
        );
        assert!(
            source.resources_returned && source.native_calls.is_baseline(),
            "{name}: {source:?}"
        );
        assert_eq!(source.native_calls.abandoned, abandoned);
        assert_eq!(source.native_calls.callback_entries, callbacks);
        assert_eq!(
            source.native_calls.prepared,
            source.native_calls.returned + abandoned
        );
        assert_eq!(source.native_calls.returned, source.native_calls.released);
        assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
        eprintln!("supplant gate {name}: Source completed");
        let jit = crate::compiled_async::run(&manifest, &artifact, cancel, shutdown, true).unwrap();
        assert_eq!(
            jit.exit_code,
            i32::from(cause.is_some()),
            "{name}: {}",
            jit.output
        );
        assert!(jit.retired && jit.rwx_mappings == 0);
        crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
        crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
        let trace = jit
            .output
            .lines()
            .find_map(|line| line.strip_prefix("RINZU_TRACE="))
            .unwrap();
        let trace: rinzu_trace::TraceDocument = serde_json::from_str(trace).unwrap();
        crate::aot::compare_async_task_traces(
            &crate::normalized_source_async_trace(&source),
            &trace,
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn compiled_managed_jit_supplants_preserve_main_turns_scopes_and_rooted_results() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-compiled-jit-supplants-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let run = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(run.exit_code, 0, "{}", run.output);
    assert!(
        run.output
            .contains("RINZU_NATIVE_CALLS=24,24,24,24,24,24,2,0,0,0,0"),
        "{}",
        run.output
    );
    assert!(
        run.output.contains("RINZU_STARTED_CHILDREN=2,5")
            && run.output.contains("RINZU_NATIVE_COMPLETIONS=4")
            && run.output.contains("\"resources\":\"baseline\""),
        "{}",
        run.output
    );
    assert!(
        run.compile.stack_map_count > 0
            && run.compile.stack_map_entry_count > 0
            && run.compile.code_size > 0
    );
    assert!(run.executable_mappings > 0 && run.retired);
    assert_eq!(run.icache_flushes, run.executable_mappings);
    assert_eq!(run.rx_transitions, run.executable_mappings);
    assert_eq!(run.rwx_mappings, 0);
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
            package_root: manifest.root.clone(),
            source_packages: Some(built.load_source_packages().unwrap()),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    let report = crate::future_identity::normalize(&source, &run.output).unwrap();
    crate::aot::compare_source_async_values_and_transport(&source, &report).unwrap();
    let jit_trace = report
        .lines()
        .find_map(|line| line.strip_prefix("RINZU_TRACE="))
        .unwrap();
    let jit_trace: rinzu_trace::TraceDocument = serde_json::from_str(jit_trace).unwrap();
    crate::aot::compare_async_task_traces(
        &crate::normalized_source_async_trace(&source),
        &jit_trace,
    )
    .unwrap();
    assert_eq!(jit_trace.producer, "rinzu-jit");
    std::fs::remove_dir_all(built.directory).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn source_supplants_queue_after_prefix_and_restart_before_child_settlement() {
    let root = std::env::temp_dir().join(format!("rinzu-source-supplant-{}", std::process::id()));
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
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert!(
        run.resources_returned && run.native_calls.is_baseline(),
        "{run:?}"
    );
    assert_eq!(run.native_calls.callback_entries, 2);
    assert_eq!(run.native_calls.prepared, 24);
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(event,
                rinzu_codegen::SourceAsyncEvent::Submitted { operation, .. }
                    if operation == "native-completion"
            ))
            .count(),
        4
    );
    assert_eq!(run.native_calls.abandoned, 0);
    assert_eq!(run.native_calls.prepared, run.native_calls.returned);
    assert_eq!(run.native_calls.returned, run.native_calls.closed);
    assert_eq!(run.native_calls.closed, run.native_calls.settled);
    assert_eq!(run.native_calls.settled, run.native_calls.read);
    assert_eq!(run.native_calls.read, run.native_calls.released);
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(
        run.heap.collections > 50 && run.heap.moved_objects > 50,
        "{run:?}"
    );
    assert_eq!(run.worker_heaps.len(), 2);
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    assert_eq!(run.events.iter().filter(|event| matches!(event,rinzu_codegen::SourceAsyncEvent::NativeEntered{export,..} if export=="loops::run")).count(),2);
    std::fs::remove_dir_all(root).unwrap();
}
