#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::*;
mod fixture {
    include!("../tests/fixtures/native_callback_sdk/setup.rs");
}
mod return_fixture {
    include!("../tests/fixtures/native_callback_return_sdk/setup.rs");
}
mod return_lifetime {
    include!("../tests/fixtures/native_callback_return_sdk/lifetime.rs");
}

fn expected(
    artifact: &rinzu_compiler::FrontendArtifact,
    built: &AotBuild,
    cancel: Option<(bool, usize)>,
) -> rinzu_codegen::SourceAsyncRun {
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
            cancel_after_polls: cancel.and_then(|(worker, limit)| (!worker).then_some(limit)),
            cancel_worker_after_polls: cancel.and_then(|(worker, limit)| worker.then_some(limit)),
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
    assert!(
        source.resources_returned
            && source.native_calls.is_baseline()
            && source.heap.userdata.is_baseline()
    );
    let mut command = Command::new(binary);
    command
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .env("ASAN_OPTIONS", "detect_leaks=0");
    if let Some((worker, limit)) = cancel {
        command.env(
            if worker {
                "RINZU_CANCEL_WORKER_AFTER_POLLS"
            } else {
                "RINZU_CANCEL_AFTER_POLLS"
            },
            limit.to_string(),
        );
    }
    let output = command.output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.success(),
        source.failure_details.is_none(),
        "{stderr}"
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

#[test]
fn generic_callbacks_aot_match_main_workers_shared_owner_and_outer_failure_scopes() {
    let root =
        std::env::temp_dir().join(format!("rinzu-aot-generic-callback-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    assert_eq!(source.native_calls.prepared, 79 * 9);
    assert_eq!(source.native_calls.callback_entries, 76 * 9);
    assert_eq!(source.native_calls.copied_bytes, 884 * 9);
    assert_eq!(source.caught_errors.len(), 4 * 9);
    let stdout = check(&built.executable, &source, None);
    assert!(
        compare_source_async_values_and_transport(
            &source,
            &stdout.replace(
                "RINZU_NATIVE_CALLS=711,711,711,711,711,711,684,7956",
                "RINZU_NATIVE_CALLS=711,711,711,711,711,711,683,7956"
            )
        )
        .is_err()
    );

    let sdk = root.join("callbacks/src/api.rinzu");
    let mut code = fs::read_to_string(&sdk).unwrap();
    code.push_str("\nasync fn parked()->State {let before=original();let returned=await spawn exercise();let after=exercise();if before!=returned||before!=after {panic(\"lostParkedRoot\");}return returned;}\nexport parked;\n");
    fs::write(&sdk, code).unwrap();
    for (name, main) in [
        (
            "parked",
            "import callbacks;async fn main(){let one=callbacks.parked();let two=callbacks.parked();let live=callbacks.exercise();let r2=await two;let r1=await one;let again=await one;if live!=r1||r1!=r2||r1!=again {panic(\"lostOwner\");}}",
        ),
        (
            "main-failure",
            "import callbacks;async fn main(){try{try {let invalid=CInt8(300);}finally {let cleanup=callbacks.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}}",
        ),
        (
            "worker-failure",
            "import callbacks;fn fail()->Int {try{try {let invalid=CInt8(300);}finally {let cleanup=callbacks.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}return 0;}async fn main(){let returned=await spawn fail();}",
        ),
    ] {
        fs::write(app.join("src/main.rinzu"), main).unwrap();
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(&app, Some(app.join("build").join(name))).unwrap();
        let source = expected(&artifact, &built, None);
        if name != "parked" {
            assert_eq!(source.native_calls.prepared, 79);
            assert_eq!(source.caught_errors.len(), 5);
            assert!(source.failure_details.is_none());
            assert_eq!(
                source.caught_errors.last().unwrap().primary.cause,
                rinzu_runtime::failure::FailureCause::Error(
                    rinzu_runtime::failure::ErrorKind::Conversion
                )
            );
        }
        check(&built.executable, &source, None);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn callback_error_groups_are_observed_on_main_and_worker_owners() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-callback-error-groups-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let sdk = root.join("callbacks/src/api.rinzu");
    let mut code = fs::read_to_string(&sdk).unwrap();
    code.insert_str(0, "import rinzu.List;\n");
    code.push_str(
        r#"
fn innerGrouped(value:CInt)->CInt {
    try all { let first=CInt8(400);let second=CInt8(500); }
    catch errors { if rinzu.List.length(errors)!=2 { panic("lost"); } }
    return value+7;
}
fn grouped(value:CInt)->CInt {
    try { let before=CInt8(300); } catch errors {}
    let nested=runRaw(innerGrouped);
    if nested!=23 {panic("wrong");}
    try { let after=CInt8(600); } catch errors {}
    return value+7;
}
fn exerciseGroups()->CInt { return runRaw(grouped); }
export exerciseGroups;
"#,
    );
    fs::write(sdk, &code).unwrap();
    let app = root.join("app");
    fs::write(app.join("src/main.rinzu"), "import callbacks;async fn main(){let mainValue=callbacks.exerciseGroups();let workerValue=await spawn callbacks.exerciseGroups();if mainValue!=23||workerValue!=23{panic(\"wrong\");}}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    assert!(source.failure_details.is_none(), "{source:#?}");
    assert_eq!(source.caught_errors.len(), 8);
    let expected_positions =
        [300, 400, 500, 600].map(|value| code.rfind(&format!("CInt8({value})")).unwrap() as u32);
    for observed in source.caught_errors.chunks_exact(4) {
        assert_eq!(
            observed
                .iter()
                .map(|failure| failure.primary.source.unwrap().start_byte)
                .collect::<Vec<_>>(),
            expected_positions
        );
    }
    check(&built.executable, &source, None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn callback_buffers_aot_match_main_workers_shared_owner_and_outer_failures() {
    let root =
        std::env::temp_dir().join(format!("rinzu-aot-callback-buffers-{}", std::process::id()));
    return_fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    assert_eq!(source.native_calls.prepared, 631);
    assert_eq!(source.native_calls.callback_entries, 504);
    assert_eq!(source.caught_errors.len(), 72);
    assert_eq!(
        source.native_calls.callback_returns,
        rinzu_codegen::CallbackReturnEvidence {
            normal: 216,
            fallback: 108,
            bytes: 2394,
            metadata_bytes: 7776,
        }
    );
    let stdout = check(&built.executable, &source, None);
    // M4 reports total allocation separately from the process-retained subset.
    // Corrupt the total actually compared with Source; replacing a missing
    // historical retained line would silently leave the report unchanged.
    let prefix = if stdout.contains("RINZU_CALLBACK_ALLOCATIONS=") {
        "RINZU_CALLBACK_ALLOCATIONS="
    } else {
        "RINZU_CALLBACK_RETURNS="
    };
    let original = format!("{prefix}216,108,2394,7776");
    assert!(stdout.contains(&original));
    for changed in [
        "215,108,2394,7776",
        "216,107,2394,7776",
        "216,108,2393,7776",
        "216,108,2394,7752",
    ] {
        assert!(
            compare_source_async_values_and_transport(
                &source,
                &stdout.replace(&original, &format!("{prefix}{changed}"))
            )
            .is_err()
        );
    }
    let sdk = root.join("returns/src/api.rinzu");
    let mut code = fs::read_to_string(&sdk).unwrap();
    code.push_str("\nasync fn parked()->String {let before=\"kept\\u{0}😀\";let returned=await spawn exercise();let after=exercise();if returned!=56||after!=56 {panic(\"lostParkedRoot\");}return before;}\nexport parked;\n");
    fs::write(&sdk, code).unwrap();
    for (name, main) in [
        (
            "parked",
            "import returns;async fn main(){let one=returns.parked();let two=returns.parked();let live=returns.exercise();let r2=await two;let r1=await one;let again=await one;if live!=56||r1!=\"kept\\u{0}😀\"||r1!=r2||r1!=again {panic(\"lostOwner\");}}",
        ),
        (
            "main-failure",
            "import returns;async fn main(){try{try {let invalid=CInt8(300);}finally {let cleanup=returns.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}}",
        ),
        (
            "worker-failure",
            "import returns;fn fail()->Int {try{try {let invalid=CInt8(300);}finally {let cleanup=returns.exercise();}}catch[conversion:Error,...rest]{if conversion.message==\"\"{panic(\"wrongCatch\");}}return 0;}async fn main(){let returned=await spawn fail();}",
        ),
    ] {
        fs::write(app.join("src/main.rinzu"), main).unwrap();
        let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
        let built = build_package(&app, Some(app.join("build").join(name))).unwrap();
        let source = expected(&artifact, &built, None);
        if name != "parked" {
            assert_eq!(source.native_calls.prepared, 70);
            assert_eq!(source.native_calls.callback_returns.bytes, 266);
            assert_eq!(source.caught_errors.len(), 9);
            assert!(source.failure_details.is_none());
            assert_eq!(
                source.caught_errors.last().unwrap().primary.cause,
                rinzu_runtime::failure::FailureCause::Error(
                    rinzu_runtime::failure::ErrorKind::Conversion
                )
            );
        }
        check(&built.executable, &source, None);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn callback_buffers_aot_survive_worker_exit_and_normal_runtime_shutdown() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-aot-callback-buffer-lifetime-{}",
        std::process::id()
    ));
    return_fixture::create(&root);
    return_lifetime::retain_until_shutdown(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    for binary in [&built.executable, &sanitized(&built)] {
        check(binary, &source, None);
        let output = Command::new(binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("RETURN_BUFFERS_AFTER_SHUTDOWN=504\n")
        );
    }
    fs::remove_dir_all(root).unwrap();
}

pub(super) fn sanitized(built: &AotBuild) -> PathBuf {
    let binary = with_suffix(&built.executable, ".sanitized");
    let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
    let output = Command::new("clang")
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
        .arg(native.join("callback_entry.c"))
        .arg(native.join("callback_template.S"))
        .arg("-I")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
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
    let output = Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(binary)
        .output()
        .unwrap();
    let actual = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success()
            && actual.contains("disable-library-validation")
            && !actual.contains("allow-jit")
            && !actual.contains("allow-unsigned-executable-memory")
    );
}

#[test]
fn generic_callbacks_aot_signed_stress_sanitizers_and_cancellation_preserve_native_returns() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-aot-generic-callback-gates-{}",
        std::process::id()
    ));
    fixture::create(&root);
    let app = root.join("app");
    let entitlements = root.join("adhoc-sdk.plist");
    fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    sign(&built.executable, &entitlements);
    check(&built.executable, &source, None);
    check(&sanitized(&built), &source, None);
    fs::write(app.join("src/main.rinzu"),"import callbacks;async fn main(){var index=0;while index<60 {let returned=callbacks.exercise();if returned!=callbacks.original(){panic(\"lost\");}index=index+1;}}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let stress = build_package(&app, Some(root.join("stress"))).unwrap();
    let source = expected(&artifact, &stress, None);
    assert_eq!(source.native_calls.prepared, 79 * 60);
    assert_eq!(source.native_calls.callback_entries, 76 * 60);
    assert_eq!(source.native_calls.copied_bytes, 884 * 60);
    assert_eq!(source.heap.userdata.created, 72 * 60);
    assert_eq!(source.caught_errors.len(), 4 * 60);
    let stdout = check(&stress.executable, &source, None);
    assert_eq!(
        parse_line(&stdout, "RINZU_USERDATA_PROCESS=").unwrap(),
        "0,0,0"
    );
    let entries =
        parse_resource_numbers(parse_line(&stdout, "RINZU_NATIVE_ENTRIES=").unwrap()).unwrap();
    assert_eq!(&entries[..3], &[1560, 4, 131072]);
    assert_eq!(&entries[4..], &[0, 0]);
    sign(&stress.executable, &entitlements);
    check(&stress.executable, &source, None);
    check(&sanitized(&stress), &source, None);

    let sdk = root.join("callbacks/src/api.rinzu");
    let mut code = fs::read_to_string(&sdk).unwrap();
    code.push_str("\nfn looping()->Int {let returned=exercise();var index=0;while index<10000 {index=index+1;}if returned!=original(){panic(\"lost\");}return index;}\nexport looping;\n");
    fs::write(&sdk, code).unwrap();
    for worker in [false, true] {
        fs::write(
            app.join("src/main.rinzu"),
            if worker {
                "import callbacks;async fn main(){let returned=await spawn callbacks.looping();}"
            } else {
                "import callbacks;async fn main(){let returned=callbacks.looping();}"
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
        let mut entered = false;
        for limit in [1, 3, 5, 10, 20, 100, 1000, 5000] {
            let cancel = Some((worker, limit));
            let source = expected(&artifact, &built, cancel);
            entered |= source.native_calls.callback_entries > 0;
            assert_eq!(source.native_calls.prepared, source.native_calls.returned);
            assert_eq!(source.native_calls.prepared, source.native_calls.released);
            check(&built.executable, &source, cancel);
            check(&sanitized, &source, cancel);
        }
        assert!(entered);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn callback_buffers_aot_signed_stress_sanitizers_and_cancellation_preserve_native_returns() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-aot-callback-buffer-gates-{}",
        std::process::id()
    ));
    return_fixture::create(&root);
    let app = root.join("app");
    let entitlements = root.join("adhoc-sdk.plist");
    fs::write(&entitlements,"<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let source = expected(&artifact, &built, None);
    sign(&built.executable, &entitlements);
    check(&built.executable, &source, None);
    check(&sanitized(&built), &source, None);
    fs::write(app.join("src/main.rinzu"),"import returns;async fn main(){var index=0;while index<60 {let returned=returns.exercise();if returned!=56 {panic(\"lost\");}index=index+1;}}").unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let stress = build_package(&app, Some(root.join("stress"))).unwrap();
    let source = expected(&artifact, &stress, None);
    assert_eq!(source.native_calls.prepared, 70 * 60);
    assert_eq!(source.native_calls.callback_entries, 56 * 60);
    assert_eq!(source.native_calls.copied_bytes, 160 * 60);
    assert_eq!(source.caught_errors.len(), 8 * 60);
    assert_eq!(
        source.native_calls.callback_returns,
        rinzu_codegen::CallbackReturnEvidence {
            normal: 1440,
            fallback: 720,
            bytes: 15960,
            metadata_bytes: 51840,
        }
    );
    let stdout = check(&stress.executable, &source, None);
    let entries =
        parse_resource_numbers(parse_line(&stdout, "RINZU_NATIVE_ENTRIES=").unwrap()).unwrap();
    assert_eq!(&entries[..3], &[780, 2, 65536]);
    assert_eq!(&entries[4..], &[0, 0]);
    sign(&stress.executable, &entitlements);
    check(&stress.executable, &source, None);
    check(&sanitized(&stress), &source, None);

    let sdk = root.join("returns/src/api.rinzu");
    let mut code = fs::read_to_string(&sdk).unwrap();
    code.push_str("\nfn looping()->Int {let returned=exercise();var index=0;while index<10000 {index=index+1;}if returned!=56 {panic(\"lost\");}return index;}\nexport looping;\n");
    fs::write(&sdk, code).unwrap();
    for worker in [false, true] {
        fs::write(
            app.join("src/main.rinzu"),
            if worker {
                "import returns;async fn main(){let returned=await spawn returns.looping();}"
            } else {
                "import returns;async fn main(){let returned=returns.looping();}"
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
        let mut entered = false;
        for limit in [1, 3, 5, 10, 20, 100, 1000, 5000] {
            let cancel = Some((worker, limit));
            let source = expected(&artifact, &built, cancel);
            entered |= source.native_calls.callback_entries > 0;
            assert_eq!(source.native_calls.prepared, source.native_calls.returned);
            assert_eq!(source.native_calls.prepared, source.native_calls.released);
            check(&built.executable, &source, cancel);
            check(&sanitized, &source, cancel);
        }
        assert!(entered);
    }
    fs::remove_dir_all(root).unwrap();
}
