#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::equivalence::{diagnostic_signature, parse_line, parse_resource_numbers};
use super::*;
use rinzu_codegen::BaselineOptions;
use std::process::Command;
#[test]
fn revised_try_all_control_cancel_shutdown_and_fatal_match_generated_c() {
    let root = std::env::temp_dir().join(format!("rinzu-revised-all-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"all_controls\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    let source = r#"import rinzu.stdio;
fn line(text:String){try{rinzu.stdio.writeLine(text);}catch unexpected{panic("failed");}}
fn work()->Int {var n=0;while n<100000{n=n+1;}return n;}
async fn child() {try {try {let done=await spawn work();} finally {line("child-cleanup");}} catch unexpected {panic("failed");}}
async fn main() {
    try all {let pending=child();var n=0;while n<100000{n=n+1;}}
    catch errors {line("catch-unreachable");}
    finally {line("finally");}
}"#;
    fs::write(root.join("src/main.rinzu"), source).unwrap();
    verify_equivalence_with_options(
        &root,
        BaselineOptions {
            cancel_after_polls: Some(40),
            ..Default::default()
        },
    )
    .unwrap();
    verify_equivalence_with_options(
        &root,
        BaselineOptions {
            shutdown_after_polls: Some(40),
            ..Default::default()
        },
    )
    .unwrap();
    fs::write(
        root.join("src/main.rinzu"),
        source.replace(
            "let pending=child();var n=0;while n<100000{n=n+1;}",
            "let pending=child();let fatal=1/0;",
        ),
    )
    .unwrap();
    verify_equivalence(&root, None).unwrap();
    fs::write(
        root.join("src/main.rinzu"),
        include_str!("../../../../tests/fixtures/programs/try-all-control/src/main.rinzu"),
    )
    .unwrap();
    verify_equivalence(&root, None).unwrap();
    let shutdown_source = r#"import rinzu.stdio;
fn line(text:String){try{rinzu.stdio.writeLine(text);}catch unexpected{panic("failed");}}
fn work()->Int {var n=0;while n<100000{n=n+1;}return n;}
async fn child(){try {try {let done=await spawn work();}finally{line("child");}}catch unexpected {panic("failed");}}
async fn window(){try all {let pending=child();let shutdown=requestShutdown();}finally{line("window");}}
async fn main(){try all {await window();}finally{line("main");}}
"#;
    fs::write(root.join("src/main.rinzu"), shutdown_source).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = build_package(&root, None).unwrap();
    let options = rinzu_codegen::SourceAsyncOptions {
        capture_stdout: true,
        record_transport_evidence: true,
        ..rinzu_codegen::SourceAsyncOptions::from_sdk_packages(
            &manifest.root,
            &artifact.sdk_packages,
        )
        .unwrap()
    };
    let mut reference =
        rinzu_codegen::run_source_async_report(&artifact.ir, "main", options).unwrap();
    let output = Command::new(&built.executable)
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    assert_eq!(output.stdout, reference.stdout);
    assert_eq!(output.status.code(), Some(i32::from(reference.exit_status)));
    let evidence = String::from_utf8(crate::verify_capture::evidence(&output).unwrap()).unwrap();
    let evidence = crate::future_identity::normalize(&reference, &evidence).unwrap();
    // Queued window and its waiting parent both receive the shutdown. The
    // first control poll can be at either site; ordinary Error locations and
    // every cleanup diagnostic remain exact in the shared comparisons.
    let failure = reference.failure_details.as_mut().unwrap();
    assert_eq!(
        failure.primary.cause,
        rinzu_runtime::failure::FailureCause::Shutdown
    );
    let source = failure.primary.source.unwrap();
    let allowed = ["requestShutdown()", "await window()"].map(|text| {
        let start = shutdown_source.find(text).unwrap() as u32;
        rinzu_runtime::failure::SourcePosition {
            file: source.file,
            start_byte: start,
            end_byte: start + text.len() as u32,
        }
    });
    assert!(allowed.contains(&source));
    let actual = parse_line(&evidence, "RINZU_DIAGNOSTICS=").unwrap();
    failure.primary.source = Some(
        *allowed
            .iter()
            .find(|source| {
                actual
                    == format!(
                        "4@{}:{}:{}",
                        source.file, source.start_byte, source.end_byte
                    )
            })
            .unwrap_or_else(|| panic!("unexpected shutdown diagnostic: {actual}")),
    );
    compare_source_async_failures(&reference, &evidence).unwrap();
    compare_source_async_values_and_transport(&reference, &evidence).unwrap();
    fs::remove_dir_all(root).unwrap();
}

mod native_length_fixture {
    include!("../../tests/fixtures/native_length_sdk/setup.rs");
}
mod native_opaque_fixture {
    include!("../../tests/fixtures/native_opaque_sdk/setup.rs");
}
mod native_data_fixture {
    include!("../../tests/fixtures/native_data_sdk/setup.rs");
}
mod native_record_fixture {
    include!("../../tests/fixtures/native_record_sdk/setup.rs");
}
#[test]
fn compiled_return_cleanup_roots_and_loop_routes_match_actual_aot() {
    let root =
        std::env::temp_dir().join(format!("rinzu-aot-return-finally-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"return_finally\"\nlanguage=\"0.2\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/main.rinzu"), r#"
fn returning()->String {
    try {try {return "retained";}finally {try {let badInner=CInt8(300);}catch [inner: Error, ...innerRest] {let copiedInner=inner.message;}}}
    finally {try {let badOuter=CInt8(400);}catch [outer: Error, ...outerRest] {let copiedOuter=outer.message;}}
}
fn recursive(depth:Int)->String {try {if depth<4 {return recursive(depth+1);}else {return "retained";}}finally {let moved=["moving","cleanup","roots"];}}
fn unpublished()->String {try {return "unpublished";}finally {let badCleanup=CInt8(200);}}
async fn ready()->Int {return 4;}
fn launch()->Future<Int> {try {return ready();}finally {let movedFuture=["opaque","Future","handle"];}}
async fn main() {
    let ordinary=returning();let nested=recursive(0);let worker=await spawn returning();
    if ordinary!="retained"||nested!=ordinary||worker!=ordinary {panic("lostRoot");}
    try {let ignored=unpublished();}catch [cleanupError: Error, ...cleanupErrorRest] {let copied=cleanupError.message;}
    let future=launch();let readyValue=await future;if readyValue!=4 {panic("wrongFuture");}
    var count=0;while count<3 {try {continue;}finally {count=count+1;}}
    while true {try {break;}finally {count=count+1;}}
    if count!=4 {panic("lostCleanup");}
}
"#).unwrap();
    let run = crate::aot::verify_equivalence(&root, None).unwrap();
    assert_eq!(run.outcome, "success");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_native_data_aot_matches_values_errors_roots_signed_and_sanitized() {
    let root = std::env::temp_dir().join(format!("rinzu-aot-native-data-{}", std::process::id()));
    native_data_fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages");
    let source = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(
                crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap(),
            ),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    let check = |binary: &Path, expected: &rinzu_codegen::SourceAsyncRun, cancel: Option<usize>| {
        let mut command = Command::new(binary);
        command
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0");
        if let Some(limit) = cancel {
            command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string());
        }
        let actual = command.output().unwrap();
        assert_eq!(
            actual.status.success(),
            expected.failure_details.is_none(),
            "{}",
            String::from_utf8_lossy(&actual.stderr)
        );
        let stdout = std::str::from_utf8(&actual.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(expected, &stdout).unwrap();
    };
    check(&built.executable, &source, None);
    assert_eq!(source.native_calls.prepared, 166);
    assert_eq!(source.caught_errors.len(), 68);
    let entitlements = root.join("adhoc-sdk-library.plist");
    fs::write(&entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let signed = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
        ])
        .arg(&entitlements)
        .arg(&built.executable)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    assert!(
        Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&built.executable)
            .status()
            .unwrap()
            .success()
    );
    check(&built.executable, &source, None);
    let sanitize = |built: &AotBuild| {
        let binary = with_suffix(&built.executable, ".sanitized");
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
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_entry.c"))
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_template.S"))
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
    };
    check(&sanitize(&built), &source, None);
    fs::write(app.join("src/main.rinzu"), "import buffers; async fn main() { let pending = spawn buffers.leaks(); let returned = await pending; }").unwrap();
    let (_, cancel_artifact) = rinzu_compiler::check_package(&app).unwrap();
    let cancelled = build_package(&app, Some(root.join("cancelled"))).unwrap();
    let cancelled_bundle = with_suffix(&cancelled.executable, ".source-packages");
    let sanitized_cancel = sanitize(&cancelled);
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let expected = rinzu_codegen::run_source_async_report(
            &cancel_artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&cancelled_bundle.join("manifest.json"))
                        .unwrap(),
                ),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(expected.resources_returned && expected.native_calls.is_baseline());
        check(&cancelled.executable, &expected, Some(limit));
        check(&sanitized_cancel, &expected, Some(limit));
    }
    let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
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
    // SAFETY: this validated Unit() fixture function runs after all physical
    // Workers and managed heaps returned, and releases only its private page.
    let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
    // SAFETY: no subsequent native read or borrowed view uses the page.
    unsafe { cleanup() };
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_native_record_aot_matches_layout_values_errors_roots_signed_and_sanitized() {
    let root = std::env::temp_dir().join(format!("rinzu-aot-native-record-{}", std::process::id()));
    native_record_fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages");
    let source = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(
                crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap(),
            ),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    let check = |binary: &Path, expected: &rinzu_codegen::SourceAsyncRun, cancel: Option<usize>| {
        let mut command = Command::new(binary);
        command
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0");
        if let Some(limit) = cancel {
            command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string());
        }
        let actual = command.output().unwrap();
        assert_eq!(
            actual.status.success(),
            expected.failure_details.is_none(),
            "{}",
            String::from_utf8_lossy(&actual.stderr)
        );
        let stdout = std::str::from_utf8(&actual.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(expected, &stdout).unwrap();
    };
    check(&built.executable, &source, None);
    assert_eq!(source.native_calls.prepared, 102);
    assert_eq!(source.caught_errors.len(), 22);
    let entitlements = root.join("adhoc-sdk-library.plist");
    fs::write(&entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let signed = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
        ])
        .arg(&entitlements)
        .arg(&built.executable)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    assert!(
        Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&built.executable)
            .status()
            .unwrap()
            .success()
    );
    check(&built.executable, &source, None);
    let sanitize = |built: &AotBuild| {
        let binary = with_suffix(&built.executable, ".sanitized");
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
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_entry.c"))
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src/callback_template.S"))
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
    };
    check(&sanitize(&built), &source, None);
    fs::write(app.join("src/main.rinzu"), "import records; async fn main() { let pending = spawn records.leaks(); let returned = await pending; }").unwrap();
    let (_, cancel_artifact) = rinzu_compiler::check_package(&app).unwrap();
    let cancelled = build_package(&app, Some(root.join("cancelled"))).unwrap();
    let cancelled_bundle = with_suffix(&cancelled.executable, ".source-packages");
    let sanitized_cancel = sanitize(&cancelled);
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let expected = rinzu_codegen::run_source_async_report(
            &cancel_artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&cancelled_bundle.join("manifest.json"))
                        .unwrap(),
                ),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(expected.resources_returned && expected.native_calls.is_baseline());
        check(&cancelled.executable, &expected, Some(limit));
        check(&sanitized_cancel, &expected, Some(limit));
    }
    let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
    let signature = artifact
        .ir
        .source_native_signatures
        .iter()
        .find(|signature| {
            signature.binding
                == rinzu_compiler::ast::NativeBinding::Symbol("record_fixture_cleanup".into())
        })
        .unwrap();
    let import = loaded
        .imports()
        .values()
        .find(|import| import.identity == signature.name)
        .unwrap();
    // SAFETY: this validated Unit() fixture function runs after all physical
    // Workers and managed heaps returned, and releases only its private page.
    let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
    // SAFETY: no subsequent native read or borrowed view uses the page.
    unsafe { cleanup() };
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_native_opaque_aot_matches_worker_values_error_cleanup_and_sanitizers() {
    let root = std::env::temp_dir().join(format!("rinzu-aot-native-opaque-{}", std::process::id()));
    native_opaque_fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages");
    let source = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(
                crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap(),
            ),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    let check = |binary: &Path, expected: &rinzu_codegen::SourceAsyncRun| {
        let output = Command::new(binary)
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(expected, &stdout).unwrap();
        assert_eq!(parse_line(&stdout, "RINZU_FAILURE=").unwrap(), "none");
    };
    check(&built.executable, &source);
    assert_eq!(source.native_calls.prepared, 351);
    assert_eq!(source.caught_errors.len(), 4);
    let entitlements = root.join("adhoc-sdk-library.plist");
    fs::write(&entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let signed = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
        ])
        .arg(&entitlements)
        .arg(&built.executable)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let verified = Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(&built.executable)
        .output()
        .unwrap();
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    check(&built.executable, &source);

    let sanitize = |built: &AotBuild| {
        let binary = with_suffix(&built.executable, ".sanitized");
        let output = Command::new("clang")
            .args([
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
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Startup finds the saved bundle relative to the original executable
        // name compiled into the runtime, so both binaries share this bundle.
        binary
    };
    check(&sanitize(&built), &source);

    fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { if objects.nullableLookup(42) != 1 { panic(\"wrongMain\"); } let a = spawn objects.nullableLookup(1); let b = spawn objects.nullableLookup(2); if await a != 1 || await b != 1 { panic(\"wrongWorker\"); } }").unwrap();
    let (_, null_artifact) = rinzu_compiler::check_package(&app).unwrap();
    let null_build = build_package(&app, Some(root.join("nullable"))).unwrap();
    let null_bundle = with_suffix(&null_build.executable, ".source-packages");
    let null_source = rinzu_codegen::run_source_async_report(
        &null_artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(
                crate::source_package_bundle::load(&null_bundle.join("manifest.json")).unwrap(),
            ),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(null_source.native_calls.prepared, 12);
    assert_eq!(null_source.caught_errors.len(), 3);
    assert_eq!(null_source.native_calls.callback_entries, 96);
    check(&null_build.executable, &null_source);
    check(&sanitize(&null_build), &null_source);

    fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { let pending = spawn objects.leaks(42); let returned = await pending; }").unwrap();
    let (_, cancel_artifact) = rinzu_compiler::check_package(&app).unwrap();
    let cancel_build = build_package(&app, Some(root.join("cancelled"))).unwrap();
    let cancel_bundle = with_suffix(&cancel_build.executable, ".source-packages");
    let mut returned_opaque = false;
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let expected = rinzu_codegen::run_source_async_report(
            &cancel_artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&cancel_bundle.join("manifest.json"))
                        .unwrap(),
                ),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(expected.resources_returned);
        assert!(expected.native_calls.is_baseline());
        assert_eq!(
            expected.native_calls.prepared,
            expected.native_calls.released
        );
        returned_opaque |= expected.native_calls.returned > 0;
        let actual = Command::new(&cancel_build.executable)
            .env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string())
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        let stdout = std::str::from_utf8(&actual.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(&expected, &stdout)
            .unwrap_or_else(|error| panic!("limit={limit}: {error}\n{stdout}"));
        assert_eq!(
            parse_line(&stdout, "RINZU_FAILURE=").unwrap(),
            if expected.failure_details.is_some() {
                "cancelled"
            } else {
                "none"
            }
        );
    }
    assert!(returned_opaque);
    let cleanup = |module: &rinzu_compiler::IrModule, bundle: &Path| {
        let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
        let signature = module
            .source_native_signatures
            .iter()
            .find(|signature| {
                signature.binding
                    == rinzu_compiler::ast::NativeBinding::Symbol("object_fixture_cleanup".into())
            })
            .unwrap();
        let import = loaded
            .imports()
            .values()
            .find(|import| import.identity == signature.name)
            .unwrap();
        // SAFETY: this exact validated Unit() binding runs only after every
        // Source Worker returned and all managed wrappers were collected.
        let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
        // SAFETY: all admitted Workers and native calls have returned.
        unsafe { cleanup() };
    };
    cleanup(&artifact.ir, &bundle);
    cleanup(&null_artifact.ir, &null_bundle);
    cleanup(&cancel_artifact.ir, &cancel_bundle);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_native_opaque_receives_prepared_results_and_recovers_null_on_main_workers() {
    let root =
        std::env::temp_dir().join(format!("rinzu-source-native-opaque-{}", std::process::id()));
    native_opaque_fixture::create(&root);
    let app = root.join("app");
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    crate::native_contract::check_artifact(&manifest, &artifact).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    let address = |symbol: &str| {
        let signature = artifact
            .ir
            .source_native_signatures
            .iter()
            .find(|signature| {
                signature.binding == rinzu_compiler::ast::NativeBinding::Symbol(symbol.into())
            })
            .unwrap();
        loaded
            .imports()
            .values()
            .find(|import| import.identity == signature.name)
            .unwrap()
            .glue_address() as usize
    };
    // SAFETY: these process-pinned v2 bindings were checked against the exact
    // ordinary CSize(CInt) and Unit() source/C prototypes above.
    let stats: unsafe extern "C" fn(i32) -> u64 =
        unsafe { std::mem::transmute(address("object_stats")) };
    // SAFETY: the cleanup binding has a checked physical Unit() signature.
    let cleanup: unsafe extern "C" fn() =
        unsafe { std::mem::transmute(address("object_fixture_cleanup")) };
    let normal = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(loaded),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(normal.failure_details, None);
    assert_eq!(normal.caught_errors.len(), 4);
    assert_eq!(
        normal.caught_errors[0].primary.cause,
        rinzu_runtime::failure::FailureCause::Error(rinzu_runtime::failure::ErrorKind::Conversion)
    );
    assert!(
        normal.caught_errors[1..]
            .iter()
            .all(|failure| failure.primary.cause
                == rinzu_runtime::failure::FailureCause::Error(
                    rinzu_runtime::failure::ErrorKind::NullNativeValue
                ))
    );
    assert!(normal.native_calls.is_baseline());
    assert_eq!(
        [
            normal.native_calls.prepared,
            normal.native_calls.returned,
            normal.native_calls.closed,
            normal.native_calls.settled,
            normal.native_calls.read,
            normal.native_calls.released,
        ],
        [351; 6]
    );
    assert_eq!(normal.native_calls.abandoned, 0);
    assert_eq!(normal.native_calls.callback_entries, 288);
    assert_eq!(normal.native_calls.copied_bytes, 1152);
    assert_eq!(normal.native_calls.prepared, normal.native_calls.released);
    assert!(normal.heap.collections > 100);
    assert_eq!(normal.worker_heaps.len(), 9);
    assert!(
        normal
            .worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    assert!(normal.resources_returned);
    // SAFETY: all physical calls/Workers have returned; fixture cleanup runs
    // explicitly only after managed wrapper teardown, never as a GC hook.
    unsafe {
        assert_eq!(stats(0), 9);
        assert_eq!(stats(1), 9);
        assert_eq!(stats(2), 9);
        assert_eq!(stats(3), 0);
        cleanup();
    }

    fs::write(app.join("src/main.rinzu"), "import objects; async fn main() { let child = spawn objects.leaks(42); try { let result = await child; } finally { let cleanupMarker = 7; } }").unwrap();
    let (_, cancelled_artifact) = rinzu_compiler::check_package(&app).unwrap();
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let run = rinzu_codegen::run_source_async_report(
            &cancelled_artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(built.load_source_packages().unwrap()),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            run.native_calls.is_baseline(),
            "{limit}: {:?}",
            run.native_calls
        );
        assert_eq!(run.native_calls.prepared, run.native_calls.returned);
        assert_eq!(run.native_calls.prepared, run.native_calls.released);
        assert_eq!(
            [
                run.native_calls.closed,
                run.native_calls.settled,
                run.native_calls.read
            ],
            [run.native_calls.prepared; 3]
        );
        assert_eq!(run.native_calls.abandoned, 0);
        assert!(run.resources_returned, "{limit}");
        // SAFETY: the cancelled Worker physically joined. Its wrapper GC
        // must not synthesize native retain/release or close the entity.
        unsafe {
            assert_eq!(stats(1), 0);
            assert_eq!(stats(2), 0);
            assert_eq!(stats(0), stats(3));
            cleanup();
        }
    }
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&built.directory).unwrap();
}

#[test]
fn source_native_length_plans_match_real_aot_workers_signed_images_and_sanitizers() {
    let root = std::env::temp_dir().join(format!("rinzu-aot-native-length-{}", std::process::id()));
    native_length_fixture::create(&root);
    let app = root.join("app");
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, None).unwrap();
    let bundle = with_suffix(&built.executable, ".source-packages");
    let loaded = crate::source_package_bundle::load(&bundle.join("manifest.json")).unwrap();
    let jit = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(loaded),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    let normal = Command::new(&built.executable)
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    assert!(
        normal.status.success(),
        "{}",
        String::from_utf8_lossy(&normal.stderr)
    );
    let stdout = std::str::from_utf8(&normal.stderr).unwrap().to_owned();
    compare_source_async_values_and_transport(&jit, &stdout).unwrap();
    assert_eq!(parse_line(&stdout, "RINZU_FAILURE=").unwrap(), "none");
    for arguments in [
        vec!["--force", "--sign", "-", "--options", "runtime"],
        vec!["--verify", "--strict"],
    ] {
        let signed = Command::new("codesign")
            .args(arguments)
            .arg(&built.executable)
            .output()
            .unwrap();
        assert!(
            signed.status.success(),
            "{}",
            String::from_utf8_lossy(&signed.stderr)
        );
    }
    let strict = Command::new(&built.executable).output().unwrap();
    assert_eq!(strict.status.code(), Some(78));
    assert!(String::from_utf8_lossy(&strict.stderr).contains("Team IDs"));
    // Ad-hoc SDK facades have no distribution Team ID. This local fixture
    // grants only the library-loading exception; no JIT/mapping entitlement.
    let local_entitlements = root.join("adhoc-library.plist");
    fs::write(&local_entitlements, "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>com.apple.security.cs.disable-library-validation</key><true/></dict></plist>").unwrap();
    let signed = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
        ])
        .arg(&local_entitlements)
        .arg(&built.executable)
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let entitlements = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(&built.executable)
        .output()
        .unwrap();
    let entitlement_text = String::from_utf8_lossy(&entitlements.stdout);
    assert!(
        entitlements.status.success()
            && entitlement_text.contains("com.apple.security.cs.disable-library-validation")
    );
    assert!(
        !entitlement_text.contains("allow-jit")
            && !entitlement_text.contains("allow-unsigned-executable-memory")
    );
    let signed = Command::new(&built.executable)
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    compare_source_async_values_and_transport(&jit, std::str::from_utf8(&signed.stderr).unwrap())
        .unwrap();
    // Every saved input is verified before the native address transaction publishes.
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    for name in ["descriptor", "library"] {
        let path = bundle.join(manifest["packages"][0][name].as_str().unwrap());
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        let saved = path.with_extension("untampered");
        fs::rename(&path, &saved).unwrap();
        fs::write(&path, changed).unwrap();
        let rejected = Command::new(&built.executable).output().unwrap();
        assert_eq!(rejected.status.code(), Some(78));
        assert!(!String::from_utf8_lossy(&rejected.stderr).contains("RINZU_RESULT="));
        fs::remove_file(&path).unwrap();
        fs::rename(&saved, &path).unwrap();
    }
    let sanitized = built.executable.parent().unwrap().join("sanitized");
    let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/src");
    let linked = Command::new("clang")
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-fno-omit-frame-pointer",
            "-pthread",
            "-I",
        ])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(&built.object)
        .arg(with_suffix(&built.executable, ".runtime.c"))
        .arg(native.join("callback_entry.c"))
        .arg(native.join("callback_template.S"))
        .arg("-o")
        .arg(&sanitized)
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let sanitized = Command::new(&sanitized)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    assert!(
        sanitized.status.success(),
        "{}",
        String::from_utf8_lossy(&sanitized.stderr)
    );
    assert!(!String::from_utf8_lossy(&sanitized.stderr).contains("runtime error:"));
    compare_source_async_values_and_transport(
        &jit,
        std::str::from_utf8(&sanitized.stderr).unwrap(),
    )
    .unwrap();
    // Cancellation is observed at managed polls after a complete physical native loop.
    fs::write(app.join("src/main.rinzu"), r#"import measure;
            fn work() -> Int { let measured = measure.measure(b"A\xff", "日本語\u{0}🙂"); var index = 0; while index < 1000 { index = index + 1; } return measured; }
            async fn main() { let returned = await spawn work(); }
        "#).unwrap();
    let (_, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let cancelled_build = build_package(&app, Some(app.join("build/cancelled"))).unwrap();
    let cancelled_bundle = with_suffix(&cancelled_build.executable, ".source-packages");
    let mut after_return = false;
    for limit in [1, 3, 5, 10, 20, 100] {
        let jit = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                source_packages: Some(
                    crate::source_package_bundle::load(&cancelled_bundle.join("manifest.json"))
                        .unwrap(),
                ),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let aot = Command::new(&cancelled_build.executable)
            .env("RINZU_CANCEL_WORKER_AFTER_POLLS", limit.to_string())
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .output()
            .unwrap();
        assert!(!aot.status.success());
        let stdout = std::str::from_utf8(&aot.stderr).unwrap().to_owned();
        compare_source_async_values_and_transport(&jit, &stdout)
            .unwrap_or_else(|error| panic!("limit={limit}: {error}\n{stdout}"));
        assert_eq!(parse_line(&stdout, "RINZU_FAILURE=").unwrap(), "cancelled");
        after_return |= jit.native_calls.callback_entries > 0;
    }
    assert!(after_return);
    fs::write(app.join("src/main.rinzu"), r#"import measure;
            async fn main() {
                let before = measure.calls(); var round = 0;
                while round < 180 { if measure.measure(b"A\xff", "日本語\u{0}🙂") != 13 { panic("wrong"); } round = round + 1; }
                if measure.calls() != before + 180 { panic("countFailure"); }
            }
        "#).unwrap();
    let (_, growth_artifact) = rinzu_compiler::check_package(&app).unwrap();
    let growth = build_package(&app, Some(app.join("build/growth"))).unwrap();
    let growth_bundle = with_suffix(&growth.executable, ".source-packages");
    let jit = rinzu_codegen::run_source_async_report(
        &growth_artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(
                crate::source_package_bundle::load(&growth_bundle.join("manifest.json")).unwrap(),
            ),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(jit.native_calls.callback_entries, 180 * 96);
    let signed = Command::new("codesign")
        .args([
            "--force",
            "--sign",
            "-",
            "--options",
            "runtime",
            "--entitlements",
        ])
        .arg(&local_entitlements)
        .arg(&growth.executable)
        .output()
        .unwrap();
    assert!(signed.status.success());
    let output = Command::new(&growth.executable)
        .env("RINZU_TRANSPORT_EVIDENCE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
    compare_source_async_values_and_transport(&jit, &stdout).unwrap();
    let entries =
        parse_resource_numbers(parse_line(&stdout, "RINZU_NATIVE_ENTRIES=").unwrap()).unwrap();
    assert_eq!(
        (entries[0], entries[1], entries[4], entries[5]),
        (540, 2, 0, 0)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn seeded_composite_workers_repeat_on_real_jit_aot_heaps_and_under_sanitizers() {
    const SEEDS: [u64; 4] = [1, 0x5eed, 0xdecafbad, 0x7fff_ffff_ffff_ffff];
    const AWAITS: [&str; 7] = [
        "let nan_copy = await received_nan;",
        "let byte_copy = await received_bytes;",
        "let text_copy = await received_text;",
        "let map_copy = await received_map;",
        "let list_copy = await received_list;",
        "let record_copy = await copy;",
        "let payload_copy = await first;",
    ];
    let fixture = include_str!("../../../codegen/tests/fixtures/worker_composite.rinzu");
    let original_awaits = format!(
        "{}\n    if nan_copy == nan_copy {{\n        rinzu.core.panic(\"bad_nan\");\n    }}\n    {}",
        AWAITS[0],
        AWAITS[1..].join("\n    ")
    );
    assert!(fixture.contains(&original_awaits));
    let directory = std::env::temp_dir().join(format!(
        "rinzu-composite-worker-seeds-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    for seed in SEEDS {
        let mut random = seed;
        let mut awaits = AWAITS;
        for index in (1..awaits.len()).rev() {
            random = random
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            awaits.swap(index, (random % (index as u64 + 1)) as usize);
        }
        let shuffled = fixture.replace(
            &original_awaits,
            &format!(
                "{}\n    if nan_copy == nan_copy {{ rinzu.core.panic(\"bad_nan\"); }}",
                awaits.join("\n    ")
            ),
        );
        let successful = format!(
            "{}\nasync fn main() {{ var iteration=0; while iteration<8 {{ let round=await repeated(); iteration=iteration+1; }} }}",
            shuffled.replace("async fn main()", "async fn repeated()")
        );
        let cancelled = format!(
            "{}\n{}",
            shuffled.replace("async fn main()", "async fn successful_main()"),
            include_str!("../../../codegen/tests/fixtures/worker_composite_cancel.rinzu")
        );
        for (source, cancel) in [
            (successful, None),
            (cancelled, Some(100 + (seed % 53) as usize)),
        ] {
            let case = format!(
                "{seed:016x}-{}",
                if cancel.is_some() {
                    "cancel"
                } else {
                    "success"
                }
            );
            fs::write(directory.join(format!("{case}.rinzu")), &source).unwrap();
            let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
                rinzu_compiler::FileId(1),
                "worker-composite-seeded.rinzu",
                source,
            ))
            .unwrap();
            let object =
                emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
            let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
            fs::write(directory.join("program.o"), &object.object).unwrap();
            fs::write(
                directory.join("runtime.c"),
                runtime_shim(&object, &[], Some(&plan), &[]),
            )
            .unwrap();
            let compile = Command::new("clang")
                .args([
                    "-std=c11",
                    "-O1",
                    "-g",
                    "-fsanitize=address,undefined",
                    "-pthread",
                    "-I",
                ])
                .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
                .arg(directory.join("runtime.c"))
                .arg(directory.join("program.o"))
                .arg("-o")
                .arg(directory.join("program"))
                .output()
                .unwrap();
            assert!(
                compile.status.success(),
                "seed={seed:x} {}",
                String::from_utf8_lossy(&compile.stderr)
            );
            // Three fresh schedules for each source retain the same seed/source;
            // actual completion interleavings are checked by the credit ledger.
            for repetition in 0..3 {
                let jit = rinzu_codegen::run_source_async_report(
                    &artifact.ir,
                    "main",
                    rinzu_codegen::SourceAsyncOptions {
                        gc_stress: true,
                        cancel_worker_after_polls: cancel,
                        record_transport_evidence: true,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert!(
                    jit.resources_returned && jit.heap.objects == 0 && jit.heap.roots == 0,
                    "seed={seed:x} repetition={repetition} {jit:?}"
                );
                assert_eq!(
                    jit.worker_heaps.len(),
                    if cancel.is_some() { 1 } else { 72 }
                );
                let mut command = Command::new(directory.join("program"));
                command
                    .env("RINZU_TRANSPORT_EVIDENCE", "1")
                    .env_remove("RINZU_CANCEL_AFTER_POLLS")
                    .env_remove("RINZU_CANCEL_WORKER_AFTER_POLLS");
                if let Some(cancel) = cancel {
                    command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", cancel.to_string());
                }
                let output = command.output().unwrap();
                let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
                fs::write(
                    directory.join(format!("{case}-{repetition}.ledger")),
                    &stdout,
                )
                .unwrap();
                assert_eq!(
                    output.status.success(),
                    jit.failure_details.is_none(),
                    "seed={seed:x} repetition={repetition} {stdout}\n{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                        && !String::from_utf8_lossy(&output.stderr).contains("runtime error:")
                );
                compare_source_async_values_and_transport(&jit, &stdout).unwrap_or_else(|error| {
                    panic!("seed={seed:x} repetition={repetition} {error}")
                });
                assert_eq!(
                    parse_line(&stdout, "RINZU_DIAGNOSTICS=").unwrap(),
                    diagnostic_signature(jit.failure_details.as_ref()),
                    "seed={seed:x} repetition={repetition}"
                );
            }
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn async_wait_cycle_diagnoses_actual_continuation_dependencies_before_resume_or_cleanup() {
    // The appended C fixture supplies every task and the actual main. Keep
    // the unused generated entry empty so unrelated worker cleanup blocks
    // do not introduce unreferenced labels under this fixture's -Werror.
    let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "wait-cycle.rinzu",
        "async fn main(){}",
    ))
    .unwrap();
    let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
    let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
    let mut source = runtime_shim(&object, &[], Some(&plan), &[])
        .replace("int main(", "int unused_source_main(");
    source.push_str(include_str!("../../tests/fixtures/async_wait_cycle.c"));
    let root = std::env::temp_dir().join(format!("rinzu-async-wait-model-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("program.o"), &object.object).unwrap();
    fs::write(root.join("runtime.c"), source).unwrap();
    for sanitized in [false, true] {
        let binary = root.join(if sanitized { "sanitized" } else { "signed" });
        let mut clang = Command::new("clang");
        clang
            .args([
                "-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(root.join("runtime.c"))
            .arg(root.join("program.o"));
        if sanitized {
            clang.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = clang.arg("-o").arg(&binary).output().unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        if !sanitized {
            for args in [
                ["--force", "--sign", "-", "--options", "runtime"].as_slice(),
                ["--verify", "--strict"].as_slice(),
            ] {
                let signed = Command::new("codesign")
                    .args(args)
                    .arg(&binary)
                    .output()
                    .unwrap();
                assert!(signed.status.success(), "{signed:?}");
            }
            let entitlements = Command::new("codesign")
                .args(["-d", "--entitlements", "-"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(entitlements.status.success() && entitlements.stdout.is_empty());
        }
        let run = |mode| {
            Command::new(&binary)
                .arg(mode)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .env("UBSAN_OPTIONS", "halt_on_error=1")
                .output()
                .unwrap()
        };
        for mode in ["normal", "pump"] {
            let normal = run(mode);
            assert!(normal.status.success(), "{mode} {normal:?}");
            assert_eq!(
                String::from_utf8_lossy(&normal.stdout).trim(),
                "ASYNC_WAIT_MODEL=baseline"
            );
        }
        for mode in ["self", "cycle"] {
            let negative = run(mode);
            assert_eq!(negative.status.code(), Some(86), "{negative:?}");
            assert!(negative.stdout.is_empty(), "{negative:?}");
            let diagnostic = String::from_utf8_lossy(&negative.stderr);
            assert!(
                diagnostic.contains("observable-wait-cycle")
                    && diagnostic.contains("source=17:31:49 owner=0x"),
                "{diagnostic}"
            );
            assert_eq!(
                diagnostic
                    .lines()
                    .filter(|line| line.starts_with("rinzu wait: "))
                    .count(),
                if mode == "self" { 1 } else { 3 }
            );
            assert!(!diagnostic.contains("runtime error:"), "{diagnostic}");
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unrelated_result_pin_survives_future_zero_gc_and_consuming_await() {
    let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "async-result-pin-identity.rinzu",
        "async fn main() {}",
    ))
    .unwrap();
    let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
    let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
    let mut source = runtime_shim(&object, &[], Some(&plan), &[])
        .replace("int main(", "int unused_source_main(");
    source.push_str(include_str!(
        "../../tests/fixtures/async_result_pin_identity.c"
    ));
    let directory = std::env::temp_dir().join(format!(
        "rinzu-async-result-pin-identity-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("program.o"), &object.object).unwrap();
    fs::write(directory.join("runtime.c"), source).unwrap();
    for flags in [
        vec!["-O2", "-Wall", "-Wextra", "-Werror"],
        vec!["-O1", "-g", "-fsanitize=address,undefined"],
    ] {
        let compile = Command::new("clang")
            .args(["-std=c11", "-pthread", "-I"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .args(flags)
            .arg(directory.join("runtime.c"))
            .arg(directory.join("program.o"))
            .arg("-o")
            .arg(directory.join("program"))
            .output()
            .unwrap();
        assert!(compile.status.success(), "{compile:?}");
        let output = Command::new(directory.join("program"))
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .env("UBSAN_OPTIONS", "halt_on_error=1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "ASYNC_RESULT_PIN_IDENTITY=4:baseline"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                && !String::from_utf8_lossy(&output.stderr).contains("runtime error:")
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn typed_worker_faults_and_late_returns_settle_actual_heaps_and_native_allocations() {
    let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1),
        "typed-worker-resources.rinzu",
        include_str!("../../tests/fixtures/typed_worker_resources.rinzu"),
    ))
    .unwrap();
    let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
    let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
    let worker = |name: &str| {
        let index = object
            .functions
            .iter()
            .find(|entry| entry.name == name)
            .unwrap()
            .index;
        plan.workers
            .iter()
            .find(|entry| entry.index == index)
            .unwrap()
    };
    let mut source = String::from(
        r#"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>
static _Atomic size_t model_native_live;
static void *model_calloc(size_t count, size_t size) {
  void *memory = calloc(count,size); if (memory) atomic_fetch_add(&model_native_live,1); return memory;
}
static inline void *model_malloc(size_t size) {
  void *memory = malloc(size); if (memory) atomic_fetch_add(&model_native_live,1); return memory;
}
static void model_free(void *memory) {
  if (memory) { assert(atomic_fetch_sub(&model_native_live,1)); free(memory); }
}
#define calloc model_calloc
#define malloc model_malloc
#define free model_free
"#,
    );
    source.push_str(
        &runtime_shim(&object, &[], Some(&plan), &[])
            .replace("int main(", "int unused_source_main("),
    );
    source.push_str(&format!(
            "\n#define MODEL_ECHO {}\n#define MODEL_ECHO_RUN rinzu_worker_run_{}\n#define MODEL_EXPANDED {}\n#define MODEL_OBSERVED {}\n#define MODEL_PACKET_TYPE {}\n",
            worker("echo").index, worker("echo").index, worker("expanded").index,
            worker("observed").index, worker("echo").parameters[0]
        ));
    source.push_str(include_str!(
        "../../tests/fixtures/typed_worker_resources.c"
    ));
    let directory = std::env::temp_dir().join(format!(
        "rinzu-typed-worker-resources-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("program.o"), &object.object).unwrap();
    fs::write(directory.join("runtime.c"), source).unwrap();
    for flags in [
        vec!["-O2", "-Wall", "-Wextra", "-Werror"],
        vec!["-O1", "-g", "-fsanitize=address,undefined"],
    ] {
        let compile = Command::new("clang")
            .args(["-std=c11", "-pthread", "-I"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .args(flags)
            .arg(directory.join("runtime.c"))
            .arg(directory.join("program.o"))
            .arg("-o")
            .arg(directory.join("program"))
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(directory.join("program")).output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::str::from_utf8(&output.stdout).unwrap().trim(),
            "TYPED_WORKER_RESOURCES=96:baseline"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                && !String::from_utf8_lossy(&output.stderr).contains("runtime error:")
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn typed_worker_heap_transport_runs_under_address_and_undefined_sanitizers() {
    let fixture = include_str!("../../../codegen/tests/fixtures/worker_composite.rinzu");
    let cancelled = format!(
        "{}\n{}",
        fixture.replace("async fn main()", "async fn successful_main()"),
        include_str!("../../../codegen/tests/fixtures/worker_composite_cancel.rinzu")
    );
    let references = format!(
        "{}\nfn verified() -> Int {{ try {{ return verify(); }} catch unexpected {{ panic(\"unexpected failure\"); return 0; }} }} async fn forwarded() -> Int {{ return await spawn verified(); }} async fn main() {{ try {{ let on_main=verify(); let reference=forwarded; let pending=reference(); let on_worker=await pending; if on_main!=42 || on_worker!=42 {{ panic(\"bad_result\"); }} }} catch unexpected {{ panic(\"failed\"); }} }}",
        include_str!("../../../codegen/tests/fixtures/function_reference.rinzu")
            .replace("fn main()", "fn unused_main()")
    );
    let directory = std::env::temp_dir().join(format!(
        "rinzu-typed-worker-sanitized-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    for (source, cancel) in [
        (fixture.to_owned(), None),
        (cancelled, Some(100)),
        (references, None),
    ] {
        let artifact = rinzu_compiler::compile_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "worker-composite-sanitized.rinzu",
            source,
        ))
        .unwrap();
        let jit = rinzu_codegen::run_source_async_report(
            &artifact.ir,
            "main",
            rinzu_codegen::SourceAsyncOptions {
                gc_stress: true,
                cancel_worker_after_polls: cancel,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let object = emit_source_async_aot_object(&artifact.ir, "main", &HashMap::new()).unwrap();
        let plan = async_aot_plan(&artifact, &object).unwrap().unwrap();
        fs::write(directory.join("program.o"), &object.object).unwrap();
        fs::write(
            directory.join("runtime.c"),
            runtime_shim(&object, &[], Some(&plan), &[]),
        )
        .unwrap();
        let compile = Command::new("clang")
            .args([
                "-std=c11",
                "-O1",
                "-g",
                "-fsanitize=address,undefined",
                "-pthread",
                "-I",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(directory.join("runtime.c"))
            .arg(directory.join("program.o"))
            .arg("-o")
            .arg(directory.join("program"))
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let mut command = Command::new(directory.join("program"));
        command
            .env("RINZU_TRANSPORT_EVIDENCE", "1")
            .env_remove("RINZU_CANCEL_AFTER_POLLS")
            .env_remove("RINZU_CANCEL_WORKER_AFTER_POLLS");
        if let Some(cancel) = cancel {
            command.env("RINZU_CANCEL_WORKER_AFTER_POLLS", cancel.to_string());
        }
        let output = command.output().unwrap();
        let stdout = std::str::from_utf8(&output.stderr).unwrap().to_owned();
        assert_eq!(
            output.status.success(),
            jit.failure_details.is_none(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("Sanitizer")
                && !String::from_utf8_lossy(&output.stderr).contains("runtime error:"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        compare_source_async_values_and_transport(&jit, &stdout).unwrap();
        assert_eq!(
            parse_line(&stdout, "RINZU_DIAGNOSTICS=").unwrap(),
            diagnostic_signature(jit.failure_details.as_ref())
        );
    }
    fs::remove_dir_all(directory).unwrap();
}
