#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
mod fixture {
    include!("../tests/fixtures/native_foreign_sdk/setup.rs");
}

#[test]
fn source_foreign_progress_rejections_name_native_metadata_without_inventing_lock_cycles() {
    const CHILD_ROOT: &str = "RINZU_FOREIGN_PROGRESS_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = crate::native_package::build_with_native_inputs(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )
        .unwrap();
        let _ = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                source_packages: Some(built.load_source_packages().unwrap()),
                package_root: manifest.root,
                ..Default::default()
            },
        )
        .unwrap();
        panic!("rejected foreign call returned");
    }
    for (name, source, reason) in fixture::negative_sources(0) {
        let root = std::env::temp_dir().join(format!(
            "rinzu-source-foreign-progress-{name}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        std::fs::write(root.join("app/src/main.rinzu"), source).unwrap();
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","source_native_foreign_tests::source_foreign_progress_rejections_name_native_metadata_without_inventing_lock_cycles","--nocapture"])
            .env(CHILD_ROOT,&root).output().unwrap();
        assert_eq!(output.status.code(), Some(86), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("FOREIGN_REJECTED_BODY"));
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains(&format!("foreign-dispatch-unavailable reason={reason} "))
                && !diagnostic.contains("source=0:")
                && diagnostic.contains("owner=0x")
                && diagnostic.contains("rinzu foreign stack: symbol="),
            "{name}: {diagnostic}"
        );
        assert!(
            !diagnostic.contains("observable-wait-cycle") && !diagnostic.contains("panic"),
            "{diagnostic}"
        );
        if name == "bounded" {
            assert!(
                diagnostic.contains("symbol=foreign_nested")
                    && diagnostic.contains("symbol=foreign_run"),
                "{diagnostic}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn source_and_compiled_foreign_callbacks_preserve_abi_origin_gc_fallback_and_children() {
    let root =
        std::env::temp_dir().join(format!("rinzu-foreign-source-jit-{}", std::process::id()));
    fixture::create(&root);
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
            package_root: manifest.root.clone(),
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
    assert_eq!(
        source
            .events
            .iter()
            .filter(|event| matches!(event, rinzu_codegen::SourceAsyncEvent::FileCompleted { .. }))
            .count(),
        8
    );
    assert_eq!(source.native_calls.callback_entries, 48);
    assert_eq!(source.native_calls.foreign.created, 72);
    assert!(source.native_calls.foreign.is_baseline());
    assert_eq!(source.native_calls.copied_bytes, 576);
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    assert!(jit.retired);
    let report = crate::future_identity::normalize(&source, &jit.output).unwrap();
    crate::aot::compare_source_async_values_and_transport(&source, &report).unwrap();
    crate::aot::compare_source_async_failures(&source, &report).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_and_compiled_foreign_cancellation_masks_callbacks_and_discards_unreadable_result() {
    use rinzu_runtime::failure::FailureCause;
    for (name, cancel, shutdown, cause) in [
        ("cancel", Some(80), None, FailureCause::Cancelled),
        ("shutdown", None, Some(80), FailureCause::Shutdown),
    ] {
        let root =
            std::env::temp_dir().join(format!("rinzu-foreign-mask-{name}-{}", std::process::id()));
        fixture::create(&root);
        std::fs::write(
            root.join("app/src/main.rinzu"),
            fixture::cancellation_source(0),
        )
        .unwrap();
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
                package_root: manifest.root.clone(),
                cancel_after_polls: cancel,
                shutdown_after_polls: shutdown,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            source.failure_details.as_ref().unwrap().primary.cause,
            cause,
            "{source:?}"
        );
        assert!(
            source.resources_returned && source.native_calls.is_baseline(),
            "{source:?}"
        );
        assert_eq!(source.native_calls.callback_entries, 48);
        assert_eq!(source.native_calls.foreign.created, 72);
        assert_eq!(source.native_calls.copied_bytes, 576);
        assert_eq!(source.native_calls.returned, 3);
        assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
        let jit = crate::compiled_async::run(&manifest, &artifact, cancel, shutdown, true).unwrap();
        assert_eq!(jit.exit_code, 1, "{}", jit.output);
        assert!(jit.retired && jit.rwx_mappings == 0);
        let report = crate::future_identity::normalize(&source, &jit.output).unwrap();
        crate::aot::compare_source_async_values_and_transport(&source, &report).unwrap();
        crate::aot::compare_source_async_failures(&source, &report).unwrap();
        let trace: rinzu_trace::TraceDocument = serde_json::from_str(
            report
                .lines()
                .find_map(|line| line.strip_prefix("RINZU_TRACE="))
                .unwrap(),
        )
        .unwrap();
        crate::aot::compare_async_task_traces(
            &crate::normalized_source_async_trace(&source),
            &trace,
        )
        .unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn source_and_compiled_foreign_records_finish_after_physical_return_and_close() {
    let root =
        std::env::temp_dir().join(format!("rinzu-foreign-return-race-{}", std::process::id()));
    fixture::create(&root);
    std::fs::write(
        root.join("app/src/main.rinzu"),
        fixture::with_mode(
            include_str!("../tests/fixtures/native_foreign_sdk/main.rinzu"),
            4,
        ),
    )
    .unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    rinzu_codegen::configure_foreign_return_probe(8);
    let source = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            package_root: manifest.root.clone(),
            record_transport_evidence: true,
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
    assert_eq!(source.native_calls.returned, 11);
    assert_eq!(source.native_calls.callback_entries, 8);
    assert_eq!(source.native_calls.copied_bytes, 136);
    assert_eq!(source.native_calls.foreign.created, 8);
    assert_eq!(
        source
            .events
            .iter()
            .filter(|event| matches!(event, rinzu_codegen::SourceAsyncEvent::FileCompleted { .. }))
            .count(),
        8
    );
    crate::compiled_async::configure_foreign_return_probe(8);
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    assert!(jit.retired && jit.rwx_mappings == 0);
    let report = crate::future_identity::normalize(&source, &jit.output).unwrap();
    crate::aot::compare_source_async_values_and_transport(&source, &report).unwrap();
    crate::aot::compare_source_async_failures(&source, &report).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn foreign_return_race_closed_entry_is_rejected_before_body() {
    const ENGINE: &str = "RINZU_FOREIGN_CLOSED_ENGINE";
    const ROOT: &str = "RINZU_FOREIGN_CLOSED_ROOT";
    if let Ok(engine) = std::env::var(ENGINE) {
        let root = std::path::PathBuf::from(std::env::var_os(ROOT).unwrap());
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        if engine == "source" {
            let built = crate::native_package::build_with_native_inputs(
                &artifact.sdk_packages,
                &manifest.root,
                &artifact.native_inputs,
            )
            .unwrap();
            rinzu_codegen::configure_foreign_return_probe(8);
            let _ = run_source_async_report(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    source_packages: Some(built.load_source_packages().unwrap()),
                    package_root: manifest.root,
                    ..Default::default()
                },
            );
        } else {
            crate::compiled_async::configure_foreign_return_probe(8);
            let _ = crate::compiled_async::run(&manifest, &artifact, None, None, false);
        }
        panic!("closed callback unexpectedly returned");
    }
    for engine in ["source", "jit"] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-foreign-closed-{engine}-{}",
            std::process::id()
        ));
        fixture::create(&root);
        std::fs::write(
            root.join("app/src/main.rinzu"),
            fixture::with_mode(
                include_str!("../tests/fixtures/native_foreign_sdk/main.rinzu"),
                5,
            ),
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "source_native_foreign_tests::foreign_return_race_closed_entry_is_rejected_before_body", "--nocapture"])
            .env(ENGINE, engine).env(ROOT, &root).output().unwrap();
        assert_eq!(output.status.code(), Some(86), "{engine}: {output:?}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("FOREIGN_REJECTED_BODY"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panic"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
