#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::*;
mod fixture {
    include!("../tests/fixtures/native_record_writeback_sdk/setup.rs");
}
mod qualifier_fixture {
    include!("../tests/fixtures/native_record_qualifiers_sdk/setup.rs");
}
mod sqlite_fixture {
    include!("../tests/fixtures/sqlite_native_data/setup.rs");
}
mod supplant_capture_fixture {
    include!("../tests/fixtures/native_supplant_capture_sdk/setup.rs");
}

fn assert_record_scratch_output(output: &str) {
    let rows: Vec<Vec<usize>> = output
        .lines()
        .filter_map(|line| line.strip_prefix("RINZU_NATIVE_RECORD_SCRATCH="))
        .map(|line| {
            line.split(',')
                .map(|value| value.parse().unwrap())
                .collect()
        })
        .collect();
    assert!(!rows.is_empty(), "{output}");
    for row in &rows {
        assert_eq!(row.len(), 4);
        assert_eq!(
            row[0], 1,
            "one cold C block, none in the 512 warm calls: {output}"
        );
        assert!(row[1] <= rinzu_runtime::data::NATIVE_RECORD_SCRATCH_LIMIT);
        assert_eq!(&row[2..], &[0, 0]);
    }
    assert_eq!(
        output.matches("RINZU_NATIVE_RECORD_DISPOSED=0,0").count(),
        rows.len()
    );
}

#[test]
fn warm_native_record_storage_is_reused_in_jit_source_async_compiled_async_and_c_aot() {
    const CHILD: &str = "RINZU_TEST_COMPILED_RECORD_SCRATCH";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let run = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
        assert_eq!(run.exit_code, 0, "{}", run.output);
        assert!(run.retired);
        return;
    }
    let root =
        std::env::temp_dir().join(format!("rinzu-warm-record-backends-{}", std::process::id()));
    fixture::create(&root);
    fs::write(root.join("app/src/main.rinzu"), r#"
        import records;
        fn execute()->Int {
            let packet=records.Packet(x:0,valid:false);
            var index=0;
            while index<512 {
                let alias=packet;
                let status=records.change(alias,1);
                if packet.x!=index+1 {panic("lost native update");}
                index=index+1;
            }
            return packet.x;
        }
        async fn main(){try {let value=execute();if value!=512 {panic("wrong result");}}catch errors{panic("unexpected native Error");}}
    "#).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = build_package(&root.join("app"), None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages").join("manifest.json");
    let load = || crate::source_package_bundle::load(&bundle).unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(load()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(512));
    assert_eq!(jit.heap_objects, 0);
    assert!(jit.native_calls.is_baseline());
    assert_eq!(jit.native_record_scratch.temporary_allocations, 5);
    assert!(
        jit.native_record_scratch.retained_bytes
            <= rinzu_runtime::data::NATIVE_RECORD_SCRATCH_LIMIT
    );
    assert_eq!(jit.native_record_scratch.active_bytes, 0);
    let reference = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(load()),
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        reference.failure_details.is_none()
            && reference.resources_returned
            && reference.native_calls.is_baseline(),
        "{reference:?}"
    );
    assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
    assert_eq!(
        reference.heap.native_record_scratch.temporary_allocations,
        5
    );
    assert_eq!(reference.heap.native_record_scratch.active_bytes, 0);
    assert!(
        reference.heap.native_record_scratch.retained_bytes
            <= rinzu_runtime::data::NATIVE_RECORD_SCRATCH_LIMIT
    );
    for binary in [&built.executable, &sanitized(&built)] {
        let output = Command::new(binary)
            .env("RINZU_NATIVE_RECORD_EVIDENCE", "1")
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_record_scratch_output(&text);
        compare_source_async_values_and_transport(&reference, &text).unwrap();
        compare_source_async_failures(&reference, &text).unwrap();
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "aot::native_record_tests::warm_native_record_storage_is_reused_in_jit_source_async_compiled_async_and_c_aot",
            "--nocapture",
        ])
        .env(CHILD, manifest.root.parent().unwrap())
        .env("RINZU_NATIVE_RECORD_EVIDENCE", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    // The embedding report captures its own evidence; private C diagnostics
    // retain the host stderr channel, which this disposable child captures.
    assert_record_scratch_output(&format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    eprintln!(
        "NATIVE_RECORD_BACKENDS={}",
        serde_json::json!({"calls":512,"rustColdTemporaryAllocations":jit.native_record_scratch.temporary_allocations,"rustIdleBytes":jit.native_record_scratch.retained_bytes,"sourceAsyncIdleBytes":reference.heap.native_record_scratch.retained_bytes,"cColdTemporaryAllocations":1,"warmTemporaryAllocations":0,"activeBytesAfterCalls":0,"domainExitBytes":0})
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sdk_supplant_callbacks_retain_private_class_capture_metadata_across_backends() {
    let root = std::env::temp_dir().join(format!("rinzu-supplant-capture-{}", std::process::id()));
    supplant_capture_fixture::create(&root);
    let app = root.join("app");
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages").join("manifest.json");
    let reference = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(crate::source_package_bundle::load(&bundle).unwrap()),
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        reference.failure_details.is_none() && reference.resources_returned,
        "{reference:?}"
    );
    assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
    check(&built.executable, &reference);
    check(&sanitized(&built), &reference);
    let compiled = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(compiled.exit_code, 0, "{}", compiled.output);
    assert!(compiled.retired);
    compare_source_async_values_and_transport(&reference, &compiled.output).unwrap();
    compare_source_async_failures(&reference, &compiled.output).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sqlite_class_observers_and_cancellation_match_c_aot_and_sanitizers() {
    use rinzu_runtime::failure::FailureCause;
    let root =
        std::env::temp_dir().join(format!("rinzu-aot-sqlite-observer-{}", std::process::id()));
    sqlite_fixture::create_observer(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = build_package(&root, None).unwrap();
    let sanitized = sanitized(&built);
    let bundle = with_suffix(&built.executable, ".source-packages").join("manifest.json");
    for cancelled in [false, true] {
        let reference = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                source_packages: Some(crate::source_package_bundle::load(&bundle).unwrap()),
                capture_stdout: true,
                gc_stress: true,
                record_transport_evidence: true,
                cancel_after_polls: cancelled.then_some(80),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reference
                .failure_details
                .as_ref()
                .map(|failure| failure.primary.cause),
            cancelled.then_some(FailureCause::Cancelled),
            "{reference:?}"
        );
        assert!(
            reference.resources_returned
                && reference.native_calls.is_baseline()
                && reference.heap.userdata.is_baseline(),
            "{reference:?}"
        );
        assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
        assert_eq!(reference.native_process.userdata, Default::default());
        if !cancelled {
            assert_eq!(reference.stdout, b"52\n");
        }
        let control = cancelled.then_some("RINZU_CANCEL_AFTER_POLLS");
        check_with_control(&built.executable, &reference, control);
        check_with_control(&sanitized, &reference, control);
        if !cancelled {
            let compiled =
                crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
            assert_eq!(compiled.exit_code, 0, "{}", compiled.output);
            assert!(compiled.retired);
            compare_source_async_values_and_transport(&reference, &compiled.output).unwrap();
            compare_source_async_failures(&reference, &compiled.output).unwrap();
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_header_record_and_callback_types_preserve_aliases_across_backends() {
    let root = std::env::temp_dir().join(format!("rinzu-record-qualifiers-{}", std::process::id()));
    qualifier_fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = build_package(&root.join("app"), None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages").join("manifest.json");
    let load = || crate::source_package_bundle::load(&bundle).unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(load()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(16), "{jit:?}");
    assert!(jit.native_calls.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    let reference = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            source_packages: Some(load()),
            capture_stdout: true,
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        reference.failure_details.is_none() && reference.native_calls.is_baseline(),
        "{reference:?}"
    );
    assert_eq!(reference.stdout, b"16\n");
    assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
    check(&built.executable, &reference);
    check(&sanitized(&built), &reference);
    let compiled = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(compiled.exit_code, 0, "{}", compiled.output);
    assert!(compiled.retired);
    compare_source_async_values_and_transport(&reference, &compiled.output).unwrap();
    compare_source_async_failures(&reference, &compiled.output).unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn sanitized(built: &AotBuild) -> PathBuf {
    let binary = with_suffix(&built.executable, ".sanitized");
    let mut command = Command::new("clang");
    command
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-omit-frame-pointer",
            "-pthread",
        ])
        .arg(&built.object)
        .arg(with_suffix(&built.executable, ".runtime.c"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_entry.c"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_template.S"))
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg("-o")
        .arg(&binary);
    if fs::read_to_string(with_suffix(&built.executable, ".runtime.c"))
        .unwrap()
        .contains("#define RINZU_AOT_HAS_SUPPLANT 1")
    {
        command
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/owner_dispatcher.c"))
            .args(["-framework", "CoreFoundation"]);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

fn check(binary: &Path, expected: &rinzu_codegen::SourceAsyncRun) {
    check_with_control(binary, expected, None);
}

fn check_with_control(
    binary: &Path,
    expected: &rinzu_codegen::SourceAsyncRun,
    control: Option<&str>,
) {
    let mut command = Command::new(binary);
    command
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .env("ASAN_OPTIONS", "detect_leaks=0");
    if let Some(control) = control {
        command.env(control, "80");
    }
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(i32::from(expected.exit_status)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected.stdout);
    let evidence = String::from_utf8(crate::verify_capture::evidence(&output).unwrap()).unwrap();
    assert!(
        !evidence.contains("ERROR: AddressSanitizer") && !evidence.contains("runtime error:"),
        "{evidence}"
    );
    compare_source_async_values_and_transport(expected, &evidence).unwrap();
    compare_source_async_failures(expected, &evidence).unwrap();
}

#[test]
fn supplant_records_aot_copy_at_entry_and_settle_before_cancel_or_shutdown() {
    use rinzu_runtime::failure::FailureCause;
    for early in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-aot-supplant-record-{}-{early}",
            std::process::id()
        ));
        fixture::create(&root);
        fs::write(
            root.join("app/src/main.rinzu"),
            include_str!("../tests/fixtures/native_record_writeback_sdk/supplant.rinzu")
                .replace("EARLY", if early { "true" } else { "false" }),
        )
        .unwrap();
        let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = build_package(&root.join("app"), None).unwrap();
        let sanitized = sanitized(&built);
        let modes: &[&str] = if early {
            &["queued-cancel"]
        } else {
            &["normal", "active-cancel", "shutdown"]
        };
        for mode in modes {
            let reference = rinzu_codegen::run_source_async_report(
                &artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    source_packages: Some(
                        crate::source_package_bundle::load(
                            &with_suffix(&built.executable, ".source-packages")
                                .join("manifest.json"),
                        )
                        .unwrap(),
                    ),
                    record_transport_evidence: true,
                    capture_stdout: true,
                    gc_stress: true,
                    cancel_after_polls: (*mode == "active-cancel").then_some(80),
                    shutdown_after_polls: (*mode == "shutdown").then_some(80),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                reference.resources_returned && reference.native_calls.is_baseline(),
                "{mode}: {reference:?}"
            );
            assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
            assert_eq!(
                reference
                    .failure_details
                    .as_ref()
                    .map(|failure| failure.primary.cause),
                match *mode {
                    "normal" => None,
                    "shutdown" => Some(FailureCause::Shutdown),
                    _ => Some(FailureCause::Cancelled),
                }
            );
            assert_eq!(
                reference.stdout,
                match *mode {
                    "normal" => b"-11\n32\n1\n".as_slice(),
                    "queued-cancel" => b"1\n0\n".as_slice(),
                    _ => b"32\n1\n".as_slice(),
                }
            );
            let control = match *mode {
                "active-cancel" => Some("RINZU_CANCEL_AFTER_POLLS"),
                "shutdown" => Some("RINZU_SHUTDOWN_AFTER_POLLS"),
                _ => None,
            };
            check_with_control(&built.executable, &reference, control);
            check_with_control(&sanitized, &reference, control);
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn shared_records_aot_write_back_aliases_and_match_busy_errors_with_sanitizers() {
    for busy in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-aot-shared-record-{}-{busy}",
            std::process::id()
        ));
        if busy {
            fixture::create_sdk_callbacks(&root);
        } else {
            fixture::create(&root);
        }
        let main = root.join("app/src/main.rinzu");
        if !busy {
            let text = fs::read_to_string(&main).unwrap()
                .replace("fn main()", "async fn main()")
                .replace("let result=execute();", "let result=execute();let workerValue=await spawn localRecordWorker();if workerValue!=10{panic(\"wrong Worker writeback\");}");
            fs::write(&main, format!("{text}\nfn localRecordWorker()->Int{{try{{let point=records.Packet(x:4,valid:false);let status=records.change(point,6);return point.x;}}catch errors{{panic(\"unexpected Worker writeback error\");return -1;}}}}")).unwrap();
        }
        let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = build_package(&root.join("app"), None).unwrap();
        let reference = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                source_packages: Some(
                    crate::source_package_bundle::load(
                        &with_suffix(&built.executable, ".source-packages").join("manifest.json"),
                    )
                    .unwrap(),
                ),
                record_transport_evidence: true,
                capture_stdout: true,
                gc_stress: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            reference.failure_details.is_none()
                && reference.resources_returned
                && reference.native_calls.is_baseline(),
            "{reference:?}"
        );
        assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
        check(&built.executable, &reference);
        check(&sanitized(&built), &reference);
        let signed = Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
        check(&built.executable, &reference);
        fs::remove_dir_all(root).unwrap();
    }
}
