use super::*;
use rinzu_bindgen::source_package::{CodeRecord, FunctionKind};
mod supplant_capture_fixture {
    include!("../../tests/fixtures/native_supplant_capture_sdk/setup.rs");
}

#[test]
fn supplant_descriptor_retains_and_validates_private_capture_contracts() {
    let root =
        std::env::temp_dir().join(format!("rinzu-supplant-descriptor-{}", std::process::id()));
    supplant_capture_fixture::create(&root);
    let (_, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let package = artifact
        .sdk_packages
        .iter()
        .find(|package| package.manifest.name == "captures")
        .unwrap();
    let directory = root.join("generated");
    std::fs::create_dir_all(&directory).unwrap();
    let built = build(
        package,
        &root.join("app"),
        &NativeCliInputs::default(),
        &directory,
    )
    .unwrap();
    let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
        &std::fs::read(&built.descriptor).unwrap(),
    )
    .unwrap();
    let model = descriptor.interface;
    let wrapper = model
        .functions
        .iter()
        .find(|function| function.identity == "captures::exercise")
        .unwrap();
    assert_eq!(wrapper.callback_sites.len(), 1);
    let site = &wrapper.callback_sites[0];
    assert_eq!(site.parameter, 0);
    assert_eq!(site.source_type, "fn(Int)->Int");
    assert_eq!(
        model
            .functions
            .iter()
            .find(|function| function.identity == site.native_function)
            .unwrap()
            .kind,
        FunctionKind::Supplant
    );
    let capture = site.capture.as_ref().unwrap();
    assert!(model.source_types.contains_key(capture));
    assert!(
        model
            .source_types
            .keys()
            .any(|name| name.ends_with("Counter"))
    );
    for target in &site.targets {
        assert!(
            model
                .functions
                .iter()
                .any(|function| function.code.as_ref() == Some(target))
        );
    }
    for mutation in 0..3 {
        let mut bad = model.clone();
        match mutation {
            0 => {
                bad.source_types.remove(capture);
            }
            1 => {
                bad.functions
                    .iter_mut()
                    .find(|function| function.identity == "captures::exercise")
                    .unwrap()
                    .callback_sites[0]
                    .parameter = 1
            }
            2 => {
                bad.functions
                    .iter_mut()
                    .find(|function| function.identity == "captures::exercise")
                    .unwrap()
                    .callback_sites[0]
                    .capture = None
            }
            _ => unreachable!(),
        }
        assert!(
            generate_source_package(bad, b"native").is_err(),
            "mutation {mutation}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
mod native_length_fixture {
    include!("../../tests/fixtures/native_length_sdk/setup.rs");
}
mod native_opaque_fixture {
    include!("../../tests/fixtures/native_opaque_sdk/setup.rs");
}

#[test]
fn ordinary_file_interface_reuses_scalar_string_bytes_and_callback_glue() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-ordinary-glue-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let _scratch = crate::verify_capture::TemporaryDirectory(root.clone());
    let entry = root.join("entry.rinzu");
    let source = rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(1), &entry,
        "native{fn scalar(x:CInt)->CInt symbol \"rn_scalar\";fn text(x:String)->Int symbol \"rn_text\";fn bytes(x:Bytes(4))->Int symbol \"rn_bytes\";fn invoke(callback:fn(CInt)->CInt)->CInt symbol \"rn_invoke\";}fn main(){}",
    )).unwrap();
    std::fs::write(&entry, source).unwrap();
    let mut context = rinzu_compiler::check_input(
        &entry,
        &rinzu_compiler::CompilationOptions {
            language: None,
            target: "aarch64-apple-darwin".into(),
        },
        &NativeCliInputs::default(),
    )
    .unwrap();
    // Exercise the same checked provider plan used by ordinary CLI inputs.
    std::fs::write(root.join("fixture.c"), "#include <stdint.h>\n#include <string.h>\nint rn_scalar(int x){return x+2;}\nint64_t rn_text(const char *x){return (int64_t)strlen(x);}\nint64_t rn_bytes(const uint8_t *x){return x[0]+x[3];}\nint rn_invoke(int (*callback)(int)){return callback(40);}\n").unwrap();
    context.metadata.native.sources.push("fixture.c".into());
    let package = ResolvedSdkPackage {
        manifest: context.metadata,
        source: context.artifact.source_interface,
        native_imports: Vec::new(),
    };
    let output = root.join("generated");
    let built = build(&package, &root, &NativeCliInputs::default(), &output).unwrap();
    let descriptor =
        rinzu_bindgen::source_package::validate_source_descriptor(&built.artifact.expected_wire)
            .unwrap();
    assert!(descriptor.interface.package.modules.is_empty());
    assert!(descriptor.interface.package.exports.is_empty());
    assert!(!root.join("rinzu.toml").exists());
    assert!(!root.join(".rinzu").exists());
    let loaded =
        rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
    let address = |name: &str| {
        loaded
            .imports()
            .values()
            .find(|value| value.identity == name)
            .unwrap()
            .glue_address()
    };
    // SAFETY: the checked fixture prototypes and all inputs match these C ABIs;
    // the generated facade and borrowed bytes remain alive through each call.
    unsafe {
        let scalar: unsafe extern "C" fn(i32) -> i32 = std::mem::transmute(address("scalar"));
        let text: unsafe extern "C" fn(*const std::ffi::c_char) -> i64 =
            std::mem::transmute(address("text"));
        let bytes: unsafe extern "C" fn(*const u8) -> i64 = std::mem::transmute(address("bytes"));
        let invoke: unsafe extern "C" fn(extern "C" fn(i32) -> i32) -> i32 =
            std::mem::transmute(address("invoke"));
        extern "C" fn callback(x: i32) -> i32 {
            x + 2
        }
        assert_eq!(scalar(40), 42);
        assert_eq!(text(c"native".as_ptr()), 6);
        assert_eq!(bytes([20_u8, 0, 0, 22].as_ptr()), 42);
        assert_eq!(invoke(callback), 42);
    }
}

#[test]
fn revised_metadata_uses_manifest_language_const_fields_and_semantic_revisions() {
    let root = std::env::temp_dir().join(format!("rinzu-revised-metadata-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("rinzu.toml"),"[package]\nname=\"tooling\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\n").unwrap();
    std::fs::write(root.join("src/api.rinzu"),"class Data {const limit:Int,value:Int fn create()->Data{return Data(limit:10,value:0);}} native {fn echo(value:CInt)->CInt symbol \"echo\";} export Data;export echo;").unwrap();
    std::fs::write(root.join("api.c"), "int echo(int value){return value;}").unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let source = artifact.source_interface.unwrap();
    let package = rinzu_compiler::ResolvedSdkPackage {
        manifest,
        source: Some(source),
        native_imports: vec![],
    };
    std::fs::create_dir_all(root.join("generated")).unwrap();
    let built = build(
        &package,
        &root,
        &NativeCliInputs::default(),
        &root.join("generated"),
    )
    .unwrap();
    let descriptor =
        rinzu_bindgen::source_package::validate_source_descriptor(&built.artifact.expected_wire)
            .unwrap();
    assert_eq!(descriptor.interface.package.language, "rinzu-0.5");
    assert_eq!((descriptor.major, descriptor.minor), (2, 6));
    let inputs = &descriptor.interface.package.build_inputs;
    assert_eq!(
        inputs["rinzu.language-semantics"],
        rinzu_compiler::LANGUAGE_SEMANTICS_REVISION
    );
    assert_eq!(
        inputs["rinzu.private-runtime-abi"],
        rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION
    );
    assert!(
        descriptor
            .interface
            .source_types
            .values()
            .any(|s| s.kind == "state" && s.const_fields == [0] && s.fields.len() == 2)
    );
    let mut changed = descriptor.interface;
    changed
        .package
        .build_inputs
        .insert("rinzu.language-semantics".into(), "previous".into());
    let changed =
        rinzu_bindgen::source_package::generate_source_package(changed, b"native").unwrap();
    assert_ne!(
        descriptor.semantic_digest,
        changed.descriptor.semantic_digest
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_jit_opaque_reservations_keep_owner_roots_and_release_null_error_ledgers() {
    let root = std::env::temp_dir().join(format!("rinzu-jit-native-opaque-{}", std::process::id()));
    native_opaque_fixture::create(&root);
    std::fs::write(root.join("app/src/main.rinzu"), r#"
        import objects;
        fn badSeed() -> Int { let bad = CInt8(999); return 42; }
        fn work() -> Int {
            let value = objects.measure(42);
            var order = 0;
            try {
                try { let missing = objects.failure(); } finally { order = order * 10 + 1; }
            } catch [nullError: Error, ...nullErrorRest] { order = order * 10 + 2; if nullError.message != "native value is NULL" { panic("wrongError"); } }
            finally { order = order * 10 + 3; }
            if order != 123 { panic("wrongFinally"); }
            try { let skipped = objects.measure(badSeed()); } catch argumentError {}
            return value;
        }
        fn execute() -> Int { return work(); }
        fn unhandled() -> Int { let missing = objects.failure(); return 999; }
        fn main() { try { if execute() != 42 { panic("wrong"); } if false { let unused = unhandled(); } } catch unexpectedEntryErrors { panic("unexpectedEntryFailure"); } }
    "#).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
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
    // SAFETY: v2 load and target Clang validated these exact CSize(CInt)
    // and Unit() fixture prototypes. Facades are pinned for process lifetime.
    let stats: unsafe extern "C" fn(i32) -> u64 =
        unsafe { std::mem::transmute(address("object_stats")) };
    // SAFETY: this checked fixture cleanup has no parameters or result.
    let cleanup: unsafe extern "C" fn() =
        unsafe { std::mem::transmute(address("object_fixture_cleanup")) };
    let options = rinzu_codegen::BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        options,
        None,
        Some(loaded),
    )
    .unwrap();
    let verify = |run: &rinzu_codegen::BaselineRun| {
        assert_eq!(run.result, Ok(42), "{run:?}");
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(
            run.caught_errors[0].primary.cause,
            rinzu_runtime::failure::FailureCause::Error(
                rinzu_runtime::failure::ErrorKind::NullNativeValue
            )
        );
        assert_eq!(
            run.caught_errors[1].primary.cause,
            rinzu_runtime::failure::FailureCause::Error(
                rinzu_runtime::failure::ErrorKind::Conversion
            )
        );
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
            [39; 6]
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
    let artifacts = built
        .source_packages
        .iter()
        .map(|package| package.artifact.clone())
        .collect::<Vec<_>>();
    let origin = std::thread::current().id();
    let workers = (0..8)
        .map(|_| {
            let ir = artifact.ir.clone();
            let artifacts = artifacts.clone();
            std::thread::spawn(move || {
                assert_ne!(origin, std::thread::current().id());
                let packages =
                    rinzu_codegen::source_package::LoadedSourcePackages::load(&artifacts).unwrap();
                rinzu_codegen::run_baseline_jit_with_package_bindings(
                    &ir,
                    "work",
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
    // SAFETY: physical Owners have finished; these are explicit fixture
    // controls, never an implicit native destructor or GC finalizer.
    unsafe {
        assert_eq!(stats(0), 9);
        assert_eq!(stats(1), 9);
        assert_eq!(stats(2), 9);
        assert_eq!(stats(3), 0);
        assert_eq!(stats(4), 18);
        cleanup();
    }
    let uncaught = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "unhandled",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(
        uncaught.result,
        Err(rinzu_codegen::BaselineFailure::Error(
            rinzu_runtime::failure::ErrorKind::NullNativeValue
        ))
    );
    assert_eq!(
        uncaught.failure_details.as_ref().unwrap().primary.cause,
        rinzu_runtime::failure::FailureCause::Error(
            rinzu_runtime::failure::ErrorKind::NullNativeValue
        )
    );
    assert_eq!(uncaught.native_calls.prepared, 1);
    assert_eq!(uncaught.native_calls.released, 1);
    assert!(
        uncaught.native_calls.is_baseline() && uncaught.heap_objects == 0,
        "{uncaught:?}"
    );
    // SAFETY: the failed call has returned, with no successful opaque value.
    unsafe {
        assert_eq!(stats(0), 0);
        assert_eq!(stats(4), 1);
        cleanup();
    }
    for limit in [1, 3, 5, 10, 20, 100, run.poll_count] {
        let cancelled = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            rinzu_codegen::BaselineOptions {
                cancel_after_polls: Some(limit),
                ..options
            },
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        let calls = cancelled.native_calls;
        assert_eq!(
            [
                calls.returned,
                calls.closed,
                calls.settled,
                calls.read,
                calls.released
            ],
            [calls.prepared; 5]
        );
        assert!(calls.is_baseline() && calls.abandoned == 0 && cancelled.heap_objects == 0);
        // SAFETY: cancellation occurred at a managed poll after real return;
        // explicit test cleanup owns the safe fixture's remaining entities.
        unsafe {
            assert_eq!(stats(0), u64::from(calls.prepared >= 1));
            assert_eq!(stats(1), u64::from(calls.prepared >= 36));
            assert_eq!(stats(2), u64::from(calls.prepared >= 3));
            assert_eq!(stats(3), stats(0) - stats(1));
            cleanup();
        }
    }
    std::fs::remove_dir_all(built.directory).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_jit_native_ledger_uses_the_exact_plan_on_main_and_physical_workers() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-compiled-native-ledger-{}",
        std::process::id()
    ));
    native_length_fixture::create(&root);
    std::fs::write(
        root.join("app/src/main.rinzu"),
        r#"
        import measure;
        fn work() -> Int { return measure.measure(b"A\xff", "日本語\u{0}🙂"); }
        fn execute() -> Int { return work(); }
        fn main() { if execute() != 13 { panic("failed"); } }
    "#,
    )
    .unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
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
        assert_eq!(run.result, Ok(13), "{run:?}");
        assert_eq!(run.heap_objects, 0);
        assert!(run.collection_count > 190 && run.root_publications > 10);
        assert_eq!(run.published_code_versions, run.retired_code_versions);
        let ledger = run.native_calls;
        assert_eq!(
            (
                ledger.prepared,
                ledger.returned,
                ledger.closed,
                ledger.settled,
                ledger.read,
                ledger.released
            ),
            (5, 5, 5, 5, 5, 5)
        );
        assert_eq!(ledger.callback_entries, 96);
        assert_eq!(ledger.copied_bytes, 416);
        assert!(ledger.is_baseline() && ledger.abandoned == 0);
        assert_eq!(run.rwx_mappings, 0);
    };
    verify(&run);
    let artifacts = built
        .source_packages
        .iter()
        .map(|package| package.artifact.clone())
        .collect::<Vec<_>>();
    let owner = std::thread::current().id();
    let workers = (0..8)
        .map(|_| {
            let ir = artifact.ir.clone();
            let artifacts = artifacts.clone();
            std::thread::spawn(move || {
                assert_ne!(std::thread::current().id(), owner);
                let packages =
                    rinzu_codegen::source_package::LoadedSourcePackages::load(&artifacts).unwrap();
                rinzu_codegen::run_baseline_jit_with_package_bindings(
                    &ir,
                    "work",
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
    let mut cancelled_after_return = false;
    for limit in [1, 3, 5, 7, 9, 15, 20, run.poll_count] {
        let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            rinzu_codegen::BaselineOptions {
                cancel_after_polls: Some(limit),
                ..options
            },
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        assert!(
            run.native_calls.is_baseline() && run.heap_objects == 0,
            "{run:?}"
        );
        let ledger = run.native_calls;
        assert_eq!(
            (
                ledger.returned,
                ledger.closed,
                ledger.settled,
                ledger.read,
                ledger.released
            ),
            (
                ledger.prepared,
                ledger.prepared,
                ledger.prepared,
                ledger.prepared,
                ledger.prepared
            )
        );
        assert!(ledger.abandoned == 0 && ledger.callback_entries.is_multiple_of(32));
        cancelled_after_return |= run.result == Err(rinzu_codegen::BaselineFailure::Cancelled)
            && ledger.callback_entries > 0;
    }
    assert!(cancelled_after_return);
    std::fs::remove_dir_all(built.directory).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_native_ledger_keeps_main_and_worker_roots_through_physical_return() {
    let root =
        std::env::temp_dir().join(format!("rinzu-source-native-ledger-{}", std::process::id()));
    native_length_fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let run = rinzu_codegen::run_source_async(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(built.load_source_packages().unwrap()),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, 0);
    assert!(run.resources_returned, "{run:?}");
    assert_eq!(run.worker_returns, 8);
    assert_eq!(run.worker_heaps.len(), 8);
    assert_eq!(run.caught_errors.len(), 1);
    assert!(
        run.heap.moved_objects > 100 && run.heap.peak_frames >= 4,
        "{run:?}"
    );
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.collections > 190
                && worker.heap.moved_objects > 100
                && worker.heap.objects == 0
                && worker.heap.roots == 0)
    );
    let ledger = run.native_calls;
    assert_eq!(
        (
            ledger.prepared,
            ledger.returned,
            ledger.closed,
            ledger.settled,
            ledger.read,
            ledger.released
        ),
        (46, 46, 46, 46, 46, 46)
    );
    assert_eq!(ledger.abandoned, 0);
    assert_eq!(ledger.callback_entries, 9 * 3 * 32);
    assert_eq!(ledger.copied_bytes, 9 * 32 * 13);
    assert!(ledger.is_baseline());
    assert!(
        run.transport_evidence
            .iter()
            .all(|credit| credit.released_at.is_some())
    );
    std::fs::write(
        root.join("app/src/main.rinzu"),
        r#"
        import measure;
        fn work() -> Int {
            let measured = measure.measure(b"A\xff", "日本語\u{0}🙂");
            var iteration = 0;
            while iteration < 1000 { iteration = iteration + 1; }
            return measured;
        }
        async fn main() { let returned = await spawn work(); }
    "#,
    )
    .unwrap();
    let (_, cancellation_artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let mut cancelled_after_native_return = false;
    for limit in [0, 1, 3, 5, 10, 20, 100] {
        let cancelled = rinzu_codegen::run_source_async_report(
            &cancellation_artifact.ir,
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
        assert_eq!(
            cancelled.failure_details.as_ref().unwrap().primary.cause,
            rinzu_runtime::failure::FailureCause::Cancelled
        );
        assert!(cancelled.resources_returned, "limit={limit}: {cancelled:?}");
        let ledger = cancelled.native_calls;
        assert_eq!(
            (
                ledger.returned,
                ledger.closed,
                ledger.settled,
                ledger.read,
                ledger.released
            ),
            (
                ledger.prepared,
                ledger.prepared,
                ledger.prepared,
                ledger.prepared,
                ledger.prepared
            )
        );
        assert!(ledger.abandoned == 0 && ledger.is_baseline());
        assert_eq!(ledger.callback_entries % 32, 0);
        cancelled_after_native_return |= ledger.callback_entries > 0;
        assert_eq!(cancelled.worker_returns, 1);
        assert!(
            cancelled
                .transport_evidence
                .iter()
                .all(|credit| credit.released_at.is_some())
        );
    }
    assert!(cancelled_after_native_return);
    // The source signature must still match the facade even if typed IR was altered.
    let mut changed = artifact.ir.clone();
    changed
        .source_native_signatures
        .iter_mut()
        .find(|signature| signature.name.ends_with("::invoke"))
        .unwrap()
        .parameters[1] = SourceAbiType::Bytes(8);
    let rejected = rinzu_codegen::run_source_async(
        &changed,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            gc_stress: true,
            source_packages: Some(built.load_source_packages().unwrap()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(
        format!("{rejected:#}").contains("signature disagrees"),
        "{rejected:#}"
    );
    std::fs::remove_dir_all(built.directory).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn callback_sites_keep_resolved_private_lengths_targets_and_exact_public_fn_types() {
    let root = std::env::temp_dir().join(format!("rinzu-callback-artifact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"callback\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsources = [\"api.c\"]\n").unwrap();
    std::fs::write(root.join("api.c"), "uint64_t invoke(uint64_t (*cb)(const unsigned char *), const unsigned char *data) { return cb(data); }\n").unwrap();
    let source = |n| {
        format!(
            r#"
        let privateWidth: CSize = {n};
        native {{ fn invoke(cb: fn(Bytes(privateWidth)) -> CSize, data: Bytes(privateWidth)) -> CSize symbol "invoke"; }}
        fn measure(data: Bytes(privateWidth)) -> CSize {{ let alias: fn(Bytes(privateWidth)) -> CSize = byteLength; return invoke(alias, data); }}
        fn four(data: Bytes(privateWidth)) -> CSize fallback 0 {{ return byteLength(data); }}
        fn withFallback(data: Bytes(privateWidth)) -> CSize {{ return invoke(four, data); }}
        fn apply(callback: fn(Bytes(privateWidth)) -> CSize, data: Bytes(privateWidth)) -> CSize {{ return invoke(callback, data); }}
        export measure; export apply; export withFallback;
    "#
        )
    };
    let mut descriptors = Vec::new();
    for n in [4, 8] {
        std::fs::write(root.join("src/api.rinzu"), source(n)).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let package = ResolvedSdkPackage {
            manifest,
            source: artifact.source_interface,
            native_imports: Vec::new(),
        };
        let directory = root.join(format!("generated-{n}"));
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &std::fs::read(&built.descriptor).unwrap(),
        )
        .unwrap();
        let expected = format!("fn(Bytes({n}))->CUInt64");
        let find = |name| {
            descriptor
                .interface
                .functions
                .iter()
                .find(|f| f.identity == name)
                .unwrap()
        };
        let apply = find("apply");
        assert_eq!(
            apply.source_type,
            format!("fn({expected},Bytes({n}))->CUInt64")
        );
        assert_eq!(apply.parameter_requirements[0], expected);
        assert_eq!(apply.callback_sites[0].parameter, 0);
        assert_eq!(apply.callback_sites[0].source_type, expected);
        let measure = find("measure");
        assert_eq!(
            measure.callback_sites[0].targets[0],
            CodeRecord {
                declaration: format!("rinzu.core::byteLength:{n}"),
                version: 1,
                fallback: None
            }
        );
        assert_eq!(
            find("withFallback").callback_sites[0].targets[0],
            find("four").code.clone().unwrap()
        );
        assert_eq!(
            find("four")
                .code
                .as_ref()
                .unwrap()
                .fallback
                .as_ref()
                .unwrap()
                .value,
            "0"
        );
        assert!(descriptor.interface.constants.is_empty());
        assert!(
            !String::from_utf8(std::fs::read(&built.descriptor).unwrap())
                .unwrap()
                .contains("privateWidth")
        );
        let header = std::fs::read_to_string(&built.contract).unwrap();
        assert!(
            header.contains("uint64_t (*a0)(const uint8_t * p0)"),
            "{header}"
        );
        assert!(header.contains("const uint8_t * a1"), "{header}");
        let loaded =
            rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
        let stubs = rinzu_codegen::source_native_abi::NativeAbiStubs::new(
            &package.source.as_ref().unwrap().signatures,
        )
        .unwrap();
        let import = loaded
            .imports()
            .values()
            .find(|import| import.identity == "invoke")
            .unwrap();
        let mut heap = rinzu_runtime::heap::DomainHeap::new(n);
        let mut data = rinzu_runtime::data::ManagedData::new(n);
        let mut roots = rinzu_runtime::data::ManagedRoots::default();
        let callback = rinzu_runtime::callback_entry::LengthCallback::prepare(
            &mut heap,
            &mut data,
            &mut roots,
            rinzu_runtime::callback_entry::LengthKind::Bytes(n),
        );
        let input = vec![255_u8; n as usize];
        assert_eq!(
            // SAFETY: the validated facade owns this exact callback/Bytes(N)
            // prototype; input and callback remain pinned on the Origin thread.
            unsafe {
                stubs.invoke(
                    "invoke",
                    import.glue_address() as usize,
                    &[callback.entry() as usize as u64, input.as_ptr() as u64],
                )
            }
            .unwrap(),
            n
        );
        assert_eq!(callback.evidence().prepared_inputs, 1);
        assert_eq!(callback.evidence().body_entries, 1);
        drop(callback);
        assert_eq!(heap.object_count(), 0);
        assert_eq!(data.root_count(), 0);
        descriptors.push(descriptor);
    }
    assert_ne!(
        descriptors[0].semantic_digest,
        descriptors[1].semantic_digest
    );
    assert_ne!(descriptors[0].import_keys, descriptors[1].import_keys);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn generated_facade_owns_typed_imports_for_source_archive_and_shared_inputs() {
    for kind in ["c", "m", "o", "a", "dylib"] {
        let root =
            std::env::temp_dir().join(format!("rinzu-source-build-{}-{kind}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/api.rinzu"),
            "native { fn add(a: CInt, b: CInt) -> CInt symbol \"api_add\"; } export add;",
        )
        .unwrap();
        std::fs::write(root.join("api.h"), "int api_add(int a, int b);\n").unwrap();
        let source_name = if kind == "m" { "api.m" } else { "api.c" };
        std::fs::write(
            root.join(source_name),
            "int api_add(int a, int b) { return a + b; }\n",
        )
        .unwrap();
        let native = match kind {
            "c" | "m" => format!("sources = [\"{source_name}\"]"),
            "o" | "a" => {
                let mut command = TargetClang::resolve().unwrap().command();
                command
                    .arg("-c")
                    .arg(root.join(source_name))
                    .arg("-o")
                    .arg(root.join("api.o"));
                run(command, "fixture object").unwrap();
                if kind == "a" {
                    let mut command = std::process::Command::new("ar");
                    command
                        .arg("rcs")
                        .arg(root.join("api.a"))
                        .arg(root.join("api.o"));
                    run(command, "fixture archive").unwrap();
                }
                format!("objects = [\"api.{kind}\"]")
            }
            _ => {
                let library = root.join("api.dylib");
                let mut command = TargetClang::resolve().unwrap().command();
                command
                    .arg("-dynamiclib")
                    .arg(root.join(source_name))
                    .arg("-install_name")
                    .arg(&library)
                    .arg("-o")
                    .arg(&library);
                run(command, "fixture shared library").unwrap();
                "objects = [\"api.dylib\"]".to_owned()
            }
        };
        std::fs::write(root.join("rinzu.toml"), format!("[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nheaders = [\"api.h\"]\n{native}\n")).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let package = ResolvedSdkPackage {
            manifest,
            source: artifact.source_interface,
            native_imports: Vec::new(),
        };
        let directory = root.join("generated");
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        let loaded =
            rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
        assert_eq!(loaded.imports().len(), 1);
        let address = loaded.imports().values().next().unwrap().glue_address();
        // SAFETY: fixture's validated contract is precisely int(int, int).
        let add: unsafe extern "C" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        // SAFETY: the validated fixture accepts any two int32_t values.
        assert_eq!(unsafe { add(17, 25) }, 42, "{kind}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn record_callback_library_only_and_service_only_builds() {
    let cases = [
        (
            "record",
            "native { record Pair { x: CInt, y: CDouble } fn read(pair: Pair, callback: fn(CInt) -> CInt) -> CInt symbol \"read_pair\"; } export Pair; export read;",
            "sources = [\"api.c\"]\nheaders = [\"api.h\"]",
            "#include <stdint.h>\nstruct Pair { int32_t x; double y; }; int32_t read_pair(const struct Pair *pair, int32_t (*callback)(int32_t)) { return callback(pair->x); }\n",
        ),
        (
            "library",
            "native { fn pid() -> CInt symbol \"getpid\"; } export pid;",
            "libraries = [\"System\"]",
            "",
        ),
        (
            "service",
            "import rinzu.timer;\nasync fn timer(delay: Int) -> Unit { await rinzu.timer.sleep(delay); }\nnative {  } export timer;",
            "",
            "",
        ),
    ];
    for (name, source, flags, c_source) in cases {
        let root =
            std::env::temp_dir().join(format!("rinzu-source-build-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/api.rinzu"), source).unwrap();
        std::fs::write(root.join("api.c"), if name == "record" { "int32_t read_pair(const struct Pair *pair, int32_t (*callback)(int32_t)) { return callback(pair->x); }\n" } else { c_source }).unwrap();
        if name == "record" {
            std::fs::write(root.join("api.h"), "#include <stdint.h>\nstruct Pair { int32_t x; double y; }; int32_t read_pair(const struct Pair *pair, int32_t (*callback)(int32_t));\n").unwrap();
        }
        std::fs::write(root.join("rinzu.toml"), format!("[package]\nname = \"{name}\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\n{flags}\n")).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let package = ResolvedSdkPackage {
            manifest,
            source: artifact.source_interface,
            native_imports: Vec::new(),
        };
        let directory = root.join("generated");
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        assert_eq!(built.artifact.library.is_none(), name == "service");
        rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
#[test]
fn generic_error_constructor_metadata_survives_source_package_generation() {
    let root = std::env::temp_dir().join(format!("rinzu-generic-errors-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname=\"genericerrors\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n").unwrap();
    let mut digests = Vec::new();
    for payload in ["payload?:T", "payload:Option<T>"] {
        std::fs::write(
            root.join("src/api.rinzu"),
            format!("error Failed<T>{{{payload}}}export Failed;"),
        )
        .unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let package = ResolvedSdkPackage {
            manifest,
            source: artifact.source_interface,
            native_imports: vec![],
        };
        let directory = root.join("generated");
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &built.artifact.expected_wire,
        )
        .unwrap();
        assert_eq!(
            descriptor.interface.source_templates["Failed"].kind,
            "error"
        );
        assert_eq!(
            descriptor.interface.source_templates["Failed"].fields[4].1,
            "Option<T>"
        );
        digests.push(descriptor.semantic_digest);
        rinzu_codegen::source_package::LoadedSourcePackages::load(&[built.artifact]).unwrap();
    }
    assert_ne!(digests[0], digests[1]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn checkout_location_and_timestamps_do_not_change_source_interface() {
    let mut descriptors = Vec::new();
    for index in 0..2 {
        let root = std::env::temp_dir().join(format!(
            "rinzu-source-digest-{}-{index}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/api.rinzu"),
            "native { fn answer() -> CInt symbol \"answer\"; } export answer;",
        )
        .unwrap();
        std::fs::write(root.join("api.h"), "int answer(void);\n").unwrap();
        std::fs::write(root.join("value header.h"), "#define ANSWER 42\n").unwrap();
        std::fs::write(
            root.join("api.c"),
            "#include \"value header.h\"\nint answer(void) { return ANSWER; }\n",
        )
        .unwrap();
        std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nheaders = [\"api.h\"]\nsources = [\"api.c\"]\ninclude-dirs = [\".\"]\n").unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let package = ResolvedSdkPackage {
            manifest,
            source: artifact.source_interface,
            native_imports: Vec::new(),
        };
        let directory = root.join("generated");
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &built.artifact.expected_wire,
        )
        .unwrap();
        assert!(
            !String::from_utf8(built.artifact.expected_wire)
                .unwrap()
                .contains(root.to_str().unwrap())
        );
        std::fs::write(root.join("value header.h"), "#define ANSWER 43\n").unwrap();
        let changed = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        let changed = rinzu_bindgen::source_package::validate_source_descriptor(
            &changed.artifact.expected_wire,
        )
        .unwrap();
        assert_ne!(descriptor.semantic_digest, changed.semantic_digest);
        assert_ne!(
            descriptor.native_artifact_digest,
            changed.native_artifact_digest
        );
        assert_eq!(descriptor.import_keys, changed.import_keys);
        descriptors.push(descriptor);
        std::fs::write(root.join("src/api.rinzu"), "// changed after checking\n").unwrap();
        let error = build(&package, &root, &NativeCliInputs::default(), &directory)
            .err()
            .unwrap();
        assert!(error.contains("changed after checking"), "{error}");
        std::fs::remove_dir_all(root).unwrap();
    }
    assert_eq!(descriptors[0], descriptors[1]);
}
#[test]
fn selected_library_content_invalidates_digest_and_rpath_resolves_facade_dependency() {
    let root = std::env::temp_dir().join(format!("rinzu-selected-library-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("native")).unwrap();
    std::fs::write(
        root.join("src/api.rinzu"),
        "native { fn answer() -> CInt symbol \"answer\"; } export answer;",
    )
    .unwrap();
    std::fs::write(root.join("rinzu.toml"), "[package]\nname = \"api\"\nlanguage = \"0.2\"\nkind = \"sdk\"\nentry = \"src/api.rinzu\"\n[native]\nsearch-paths = [\"native\"]\nlibraries = [\"answer\"]\n").unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let package = ResolvedSdkPackage {
        manifest,
        source: artifact.source_interface,
        native_imports: Vec::new(),
    };
    let mut descriptors = Vec::new();
    let mut final_artifact = None;
    for answer in [42, 43] {
        let source = root.join("native/answer.c");
        std::fs::write(
            &source,
            format!("int answer(void) {{ return {answer}; }}\n"),
        )
        .unwrap();
        let mut command = TargetClang::resolve().unwrap().command();
        command
            .arg("-dynamiclib")
            .arg(&source)
            .arg("-install_name")
            .arg("@rpath/libanswer.dylib")
            .arg("-o")
            .arg(root.join("native/libanswer.dylib"));
        run(command, "fixture library").unwrap();
        let directory = root.join(format!("generated-{answer}"));
        std::fs::create_dir_all(&directory).unwrap();
        let built = build(&package, &root, &NativeCliInputs::default(), &directory).unwrap();
        let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
            &built.artifact.expected_wire,
        )
        .unwrap();
        assert!(
            descriptor
                .interface
                .package
                .build_inputs
                .contains_key("link:sdk:native/libanswer.dylib")
        );
        descriptors.push(descriptor);
        final_artifact = Some(built.artifact);
    }
    assert_ne!(
        descriptors[0].semantic_digest,
        descriptors[1].semantic_digest
    );
    assert_ne!(
        descriptors[0].native_artifact_digest,
        descriptors[1].native_artifact_digest
    );
    assert_eq!(descriptors[0].import_keys, descriptors[1].import_keys);
    let loaded =
        rinzu_codegen::source_package::LoadedSourcePackages::load(&[final_artifact.unwrap()])
            .unwrap();
    let address = loaded.imports().values().next().unwrap().glue_address();
    // SAFETY: the fixture contract is exactly int(void), checked before publication.
    let answer: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    // SAFETY: validated zero-argument function with a scalar result.
    assert_eq!(unsafe { answer() }, 43);
    std::fs::remove_dir_all(root).unwrap();
}
