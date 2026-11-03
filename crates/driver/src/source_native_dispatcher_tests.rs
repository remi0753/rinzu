#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{SourceAsyncOptions, run_source_async_report};
mod fixture {
    include!("../tests/fixtures/native_dispatcher_sdk/setup.rs");
}
#[test]
fn source_and_compiled_main_turns_complete_io_workers_and_gc_inside_native_loop() {
    let root = std::env::temp_dir().join(format!("rinzu-main-turns-{}", std::process::id()));
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
    assert_eq!(source.native_calls.returned, 7);
    assert_eq!(source.native_calls.callback_entries, 1);
    assert_eq!(source.native_calls.foreign.created, 1);
    assert_eq!(source.worker_returns, 2);
    let returned=source.events.iter().rposition(|event|matches!(event,rinzu_codegen::SourceAsyncEvent::NativeLeft {export,..} if export.ends_with("run"))).unwrap();
    assert_eq!(
        source.events[..returned]
            .iter()
            .filter(|event| matches!(event, rinzu_codegen::SourceAsyncEvent::FileCompleted { .. }))
            .count(),
        2
    );
    assert_eq!(
        source.events[..returned]
            .iter()
            .filter(|event| matches!(
                event,
                rinzu_codegen::SourceAsyncEvent::WorkerCompleted { .. }
            ))
            .count(),
        2
    );
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    assert!(jit.retired && jit.rwx_mappings == 0);
    crate::aot::compare_source_async_values_and_transport(&source, &jit.output).unwrap();
    crate::aot::compare_source_async_failures(&source, &jit.output).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn actual_source_activation_warnings_preserve_callback_and_child_results() {
    const CHILD_ROOT: &str = "RINZU_SOURCE_DIAGNOSTIC_ROOT";
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
        let expected_calls = std::env::var("RINZU_SOURCE_DIAGNOSTIC_CALLS")
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(source.native_calls.returned, expected_calls);
        assert_eq!(source.worker_returns, 2);
        assert_eq!(source.native_calls.foreign.retired, 1);
        println!("SOURCE_DIAGNOSTIC_BASELINE");
        return;
    }
    mod diagnostic_source {
        include!("../tests/fixtures/native_dispatcher_sdk/diagnostics_source.rs");
    }
    let root = std::env::temp_dir().join(format!(
        "rinzu-source-activation-diagnostics-{}",
        std::process::id()
    ));
    fixture::create(&root);
    for variant in ["normal", "packet", "main"] {
        let source = match variant {
            "main" => diagnostic_source::main_ready_source(0),
            "packet" => diagnostic_source::source(0)
                .replace("dispatcher.delay(120)", "dispatcher.delay(1200)"),
            _ => diagnostic_source::source(0),
        };
        std::fs::write(root.join("app/src/main.rinzu"), source).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","source_native_dispatcher_tests::actual_source_activation_warnings_preserve_callback_and_child_results","--nocapture"])
        .env(CHILD_ROOT,&root).env("RINZU_SOURCE_DIAGNOSTIC_CALLS",if variant=="main" {"12"} else {"11"}).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("SOURCE_DIAGNOSTIC_BASELINE"));
        let warnings = String::from_utf8_lossy(&output.stderr);
        for expected in [
            "long-native symbol=dispatcher_run ",
            "long-native symbol=dispatcher_delay ",
            "long-native symbol=dispatcher_worker_delay ",
            "long-managed symbol=callback ",
            "long-managed symbol=flow ",
            "long-managed symbol=calculate ",
            "foreign-result-wait symbol=callback ",
        ] {
            assert!(
                warnings.contains(expected),
                "missing {expected}: {warnings}"
            );
        }
        for warning in warnings
            .lines()
            .filter(|line| line.starts_with("rinzu warning:"))
        {
            assert!(!warning.contains("source=0:"), "{warning}");
            assert!(
                warning.contains("owner=0x")
                    && warning.contains("depth=")
                    && warning.contains("elapsed_ms=")
                    && warning.contains("stack="),
                "{warning}"
            );
        }
        if variant == "packet" {
            let warning = warnings
                .lines()
                .find(|line| line.contains("owner-stall symbol=reactor.file-read-size "))
                .unwrap_or_else(|| panic!("{warnings}"));
            assert!(
                warning.contains("dispatcher_delay") && warning.contains("callback"),
                "{warning}"
            );
        } else if variant == "main" {
            let warning = warnings
                .lines()
                .find(|line| line.contains("owner-stall symbol=observeReady "))
                .unwrap_or_else(|| panic!("{warnings}"));
            assert!(
                warning.contains("dispatcher_delay") && warning.contains("holdReady"),
                "{warning}"
            );
            assert!(
                !warnings.contains("owner-stall symbol=reactor.file-read-size "),
                "{warnings}"
            );
        } else {
            assert!(!warnings.contains("owner-stall "), "{warnings}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
