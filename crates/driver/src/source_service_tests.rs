#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod fixture {
    include!("../tests/fixtures/native_services_sdk/setup.rs");
}
#[allow(dead_code)]
#[path = "../tests/support/scratch.rs"]
mod scratch;
use rinzu_codegen::{SourceAsyncEvent, SourceAsyncOptions};
use rinzu_runtime::failure::{ErrorKind, FailureCause};
mod io_fixture {
    include!("../../../sdk/io/tests/package.rs");
}

#[test]
fn ordinary_io_sdk_loads_its_generated_service_descriptor_in_source_and_compiled_jit() {
    let root = std::env::temp_dir().join(format!("rinzu-io-source-jit-{}", std::process::id()));
    let app = io_fixture::create(&root);
    // Exercise the shipped runtime headers through the sealed standard module.
    // The older fixture's path dependency cannot replace a toolchain module.
    std::fs::write(
        app.join("rinzu.toml"),
        "[package]\nname=\"io_source_app\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    assert_eq!(artifact.sdk_packages.len(), 1);
    assert!(artifact.sdk_packages[0].native_imports.is_empty());
    assert!(artifact.sdk_packages[0].source.is_some());
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    assert_eq!(built.source_packages.len(), 1);
    assert!(built.artifacts.is_empty());
    let source = rinzu_codegen::run_source_async_report(
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
    assert!(source.resources_returned && source.failure_details.is_none());
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
    crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn closed_timer_yields_rejects_negative_and_keeps_gc_roots_in_source_and_compiled_jit() {
    let root =
        std::env::temp_dir().join(format!("rinzu-closed-timer-source-{}", std::process::id()));
    fixture::create(&root, false);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let source = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(built.load_source_packages().unwrap()),
            package_root: manifest.root.clone(),
            record_transport_evidence: true,
            completion_timeout: std::time::Duration::from_millis(5),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        source.failure_details.is_none() && source.resources_returned,
        "{source:?}"
    );
    assert_eq!(source.caught_errors.len(), 1);
    assert_eq!(
        source.caught_errors[0].primary.cause,
        FailureCause::Error(ErrorKind::Conversion)
    );
    assert!(source.caught_errors[0].primary.source.is_some());
    assert_eq!(
        source.worker_returns, 0,
        "timers never consume CPU Worker admission"
    );
    assert_eq!(source.transport.peak_worker_domains, 0);
    assert!(source.heap.collections > 10);
    let submitted = source
        .events
        .iter()
        .filter_map(|event| match event {
            SourceAsyncEvent::Submitted { future, operation } if operation == "timer" => {
                Some(*future)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(submitted.len(), 2, "negative timers start no helper");
    let completed = source
        .events
        .iter()
        .filter_map(|event| match event {
            SourceAsyncEvent::TimerCompleted { future } => Some(*future),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        completed,
        [submitted[1], submitted[0]],
        "zero timer progresses while slow timer is pending"
    );
    for future in submitted {
        let suspended = source.events.iter().position(|event| matches!(event, SourceAsyncEvent::Suspended { future: actual, operation, .. } if *actual==future && operation=="timer")).unwrap();
        let completed = source.events.iter().position(|event| matches!(event, SourceAsyncEvent::TimerCompleted { future: actual } if *actual==future)).unwrap();
        assert!(
            suspended < completed,
            "including zero milliseconds yields before completion"
        );
    }
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
    crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
    assert!(jit.output.contains("RINZU_STARTED_TIMERS=2"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelled_and_shutdown_timers_wait_for_physical_return_before_origin_scope_settles() {
    let root = scratch::Scratch::new("rinzu-closed-timer-cancel");
    fixture::create(root.path(), true);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.path().join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    for shutdown in [false, true] {
        let (cancel_limit, shutdown_limit) = if shutdown {
            (None, Some(fixture::CONTROL_POLLS))
        } else {
            (Some(fixture::CONTROL_POLLS), None)
        };
        let started = std::time::Instant::now();
        let source = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                package_root: manifest.root.clone(),
                cancel_after_polls: cancel_limit,
                shutdown_after_polls: shutdown_limit,
                record_transport_evidence: true,
                completion_timeout: std::time::Duration::from_millis(5),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(started.elapsed() >= fixture::TIMER_DURATION);
        assert_eq!(
            source.failure_details.as_ref().unwrap().primary.cause,
            if shutdown {
                FailureCause::Shutdown
            } else {
                FailureCause::Cancelled
            }
        );
        assert!(source.resources_returned && source.heap.collections > 10);
        let completed: Vec<_> = source
            .events
            .iter()
            .filter_map(|event| match event {
                SourceAsyncEvent::TimerCompleted { future } => Some(*future),
                _ => None,
            })
            .collect();
        assert_eq!(source.cancelled_children.len(), 1, "{:?}", source.events);
        assert_eq!(completed, source.cancelled_children);
        let jit =
            crate::compiled_async::run(&manifest, &artifact, cancel_limit, shutdown_limit, true)
                .unwrap();
        assert_eq!(jit.exit_code, 1, "{}", jit.output);
        crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
        crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
    }
}
