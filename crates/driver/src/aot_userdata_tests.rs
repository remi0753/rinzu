#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::*;
mod fixture {
    include!("../tests/fixtures/native_userdata_sdk/setup.rs");
}
mod context_fixture {
    include!("../tests/fixtures/native_context_records_sdk/setup.rs");
}

#[test]
fn supplant_context_results_and_discarded_results_match_aot_and_sanitizers() {
    use rinzu_runtime::failure::FailureCause;
    for early in [false, true] {
        eprintln!("building supplant context fixture: early={early}");
        let root = std::env::temp_dir().join(format!(
            "rinzu-aot-context-supplant-{}-{early}",
            std::process::id()
        ));
        context_fixture::create_supplant(&root, early);
        let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
        let built = build_package(&root.join("app"), None).unwrap();
        let sanitized = sanitized(&built);
        let modes: &[&str] = if early {
            &["queued-cancel"]
        } else {
            &["normal", "active-cancel", "shutdown"]
        };
        for mode in modes {
            eprintln!("checking supplant context: {mode}");
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
                    capture_stdout: true,
                    gc_stress: true,
                    record_transport_evidence: true,
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
            assert_eq!(reference.native_process.userdata, Default::default());
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
                    "normal" => b"10\n41\n33\n1\n".as_slice(),
                    "queued-cancel" => b"20\n1\n0\n".as_slice(),
                    _ => b"31\n33\n1\n".as_slice(),
                }
            );
            let control = match *mode {
                "active-cancel" => Some(("RINZU_CANCEL_AFTER_POLLS", 80)),
                "shutdown" => Some(("RINZU_SHUTDOWN_AFTER_POLLS", 80)),
                _ => None,
            };
            check_environment(&built.executable, &reference, control);
            check_environment(&sanitized, &reference, control);
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn context_results_and_null_errors_match_c_aot_and_sanitizers_after_gc() {
    let root =
        std::env::temp_dir().join(format!("rinzu-aot-context-results-{}", std::process::id()));
    context_fixture::create_results(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
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
            capture_stdout: true,
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(reference.stdout, b"95\n");
    assert!(reference.failure_details.is_none());
    assert_eq!(reference.caught_errors.len(), 2);
    assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
    assert_eq!(reference.native_process.userdata, Default::default());
    check(&built.executable, &reference, None);
    check(&sanitized(&built), &reference, None);
    let compiled = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(compiled.exit_code, 0, "{}", compiled.output);
    assert!(compiled.retired);
    compare_source_async_values_and_transport(&reference, &compiled.output).unwrap();
    compare_source_async_failures(&reference, &compiled.output).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn structural_contexts_with_classes_native_references_and_closures_match_aot_and_sanitizers() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-aot-context-references-{}",
        std::process::id()
    ));
    context_fixture::create(&root);
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
            capture_stdout: true,
            gc_stress: true,
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(reference.stdout, b"25\n");
    assert!(reference.failure_details.is_none());
    check(&built.executable, &reference, None);
    check(&sanitized(&built), &reference, None);
    std::fs::remove_dir_all(root).unwrap();
}

fn expected(
    artifact: &rinzu_compiler::FrontendArtifact,
    built: &AotBuild,
    cancel: Option<(bool, usize)>,
) -> rinzu_codegen::SourceAsyncRun {
    let worker = cancel.is_some_and(|(worker, _)| worker);
    let limit = cancel.map(|(_, limit)| limit);
    rinzu_codegen::run_source_async_report(
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
            cancel_after_polls: (!worker).then_some(limit).flatten(),
            cancel_worker_after_polls: worker.then_some(limit).flatten(),
            ..Default::default()
        },
    )
    .unwrap()
}

fn check(
    binary: &Path,
    source: &rinzu_codegen::SourceAsyncRun,
    cancel: Option<(bool, usize)>,
) -> String {
    check_environment(
        binary,
        source,
        cancel.map(|(worker, limit)| {
            (
                if worker {
                    "RINZU_CANCEL_WORKER_AFTER_POLLS"
                } else {
                    "RINZU_CANCEL_AFTER_POLLS"
                },
                limit,
            )
        }),
    )
}

fn check_environment(
    binary: &Path,
    source: &rinzu_codegen::SourceAsyncRun,
    control: Option<(&str, usize)>,
) -> String {
    assert!(
        source.resources_returned
            && source.native_calls.is_baseline()
            && source.heap.userdata.is_baseline()
    );
    let mut command = Command::new(binary);
    command
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .env("ASAN_OPTIONS", "detect_leaks=0");
    if let Some((variable, limit)) = control {
        command.env(variable, limit.to_string());
    }
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.stdout, source.stdout);
    assert_eq!(
        output.status.success(),
        source.failure_details.is_none(),
        "control={control:?}: {stderr}"
    );
    assert!(
        !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
        "{stderr}"
    );
    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
    compare_source_async_values_and_transport(source, &stdout).unwrap();
    assert_eq!(
        parse_line(&stdout, "RINZU_DIAGNOSTICS=").unwrap(),
        diagnostic_signature(source.failure_details.as_ref())
    );
    let caught = source
        .caught_errors
        .iter()
        .map(|error| diagnostic_signature(Some(error)))
        .collect::<Vec<_>>()
        .join(";");
    assert_eq!(parse_line(&stdout, "RINZU_CAUGHT_ERRORS=").unwrap(), caught);
    stdout
}

fn sanitized(built: &AotBuild) -> std::path::PathBuf {
    let binary = with_suffix(&built.executable, ".sanitized");
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
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
        .arg(runtime.join("callback_entry.c"))
        .arg(runtime.join("callback_template.S"))
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg("-o")
        .arg(&binary);
    if fs::read_to_string(with_suffix(&built.executable, ".runtime.c"))
        .unwrap()
        .contains("#define RINZU_AOT_HAS_SUPPLANT 1")
    {
        command
            .arg(runtime.join("owner_dispatcher.c"))
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

fn sign(binary: &Path, entitlements: &Path) {
    let output = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
        ])
        .arg(entitlements)
        .arg(binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(binary)
            .status()
            .unwrap()
            .success()
    );
    let actual = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(binary)
        .output()
        .unwrap();
    assert!(actual.status.success());
    let actual = String::from_utf8(actual.stdout).unwrap();
    assert!(
        actual.contains("disable-library-validation")
            && !actual.contains("allow-jit")
            && !actual.contains("allow-unsigned-executable-memory")
    );
}

#[test]
fn userdata_aot_signed_stress_sanitizers_and_cancellation_keep_actual_returns() {
    let root =
        std::env::temp_dir().join(format!("rinzu-aot-userdata-gates-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    assert_eq!(source.native_calls.prepared, 119);
    assert_eq!(source.heap.userdata.created, 90);
    check(&built.executable, &source, None);
    let entitlements = root.join("adhoc-sdk.plist");
    fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    sign(&built.executable, &entitlements);
    check(&built.executable, &source, None);
    check(&sanitized(&built), &source, None);

    fs::write(app.join("src/main.rinzu"),"import userdata; async fn main() {var index=0;while index<60 {let value=userdata.exercise();let returned=userdata.returning();if value!=returned {panic(\"wrong\");}index=index+1;}}\n").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let stress = build_package(&app, Some(root.join("stress"))).unwrap();
    let source = expected(&artifact, &stress, None);
    assert_eq!(source.native_calls.prepared, 780);
    assert_eq!(source.heap.userdata.created, 3900);
    assert_eq!(source.native_calls.callback_entries, 24960);
    assert_eq!(source.native_calls.copied_bytes, 99840);
    let stdout = check(&stress.executable, &source, None);
    assert_eq!(
        parse_line(&stdout, "RINZU_USERDATA_PROCESS=").unwrap(),
        "0,0,0"
    );
    let entries =
        parse_resource_numbers(parse_line(&stdout, "RINZU_NATIVE_ENTRIES=").unwrap()).unwrap();
    assert_eq!(&entries[..3], &[780, 2, 65536]);
    assert_eq!(&entries[4..], &[0, 0]);
    assert!(entries[3] > 32768);
    sign(&stress.executable, &entitlements);
    check(&stress.executable, &source, None);
    check(&sanitized(&stress), &source, None);

    for worker in [false, true] {
        fs::write(
            app.join("src/main.rinzu"),
            if worker {
                "import userdata; async fn main() {let value=await spawn userdata.leaks();}\n"
            } else {
                "import userdata; async fn main() {let value=userdata.leaks();}\n"
            },
        )
        .unwrap();
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(
            &app,
            Some(root.join(if worker {
                "worker-cancel"
            } else {
                "main-cancel"
            })),
        )
        .unwrap();
        let sanitized = sanitized(&built);
        let mut returned = false;
        for limit in [1, 3, 5, 10, 20, 100, 1000] {
            let source = expected(&artifact, &built, Some((worker, limit)));
            assert_eq!(source.native_calls.prepared, source.native_calls.returned);
            assert_eq!(source.native_calls.prepared, source.native_calls.released);
            returned |= source.native_calls.returned > 0;
            check(&built.executable, &source, Some((worker, limit)));
            check(&sanitized, &source, Some((worker, limit)));
        }
        assert!(returned);
    }
    fs::remove_dir_all(root).unwrap();
}
