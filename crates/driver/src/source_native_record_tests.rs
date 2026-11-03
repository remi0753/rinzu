#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_package_bindings,
    run_source_async_report, source_native_record_layouts,
};
use std::sync::Arc;
mod fixture {
    include!("../tests/fixtures/native_record_sdk/setup.rs");
}
mod writeback_fixture {
    include!("../tests/fixtures/native_record_writeback_sdk/setup.rs");
}

#[test]
fn source_async_invalid_record_writeback_exits_86_without_managed_resume() {
    const CHILD: &str = "RINZU_TEST_SOURCE_WRITEBACK_FATAL";
    if let Some(root) = std::env::var_os(CHILD) {
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
                gc_stress: true,
                ..Default::default()
            },
        );
        panic!("resumed after invalid source async writeback");
    }
    let root = std::env::temp_dir().join(format!(
        "rinzu-source-writeback-fatal-{}",
        std::process::id()
    ));
    writeback_fixture::create(&root);
    let native = root.join("records/api.c");
    let contents = std::fs::read_to_string(&native).unwrap();
    std::fs::write(
        native,
        format!(
            "#include <string.h>\n{}",
            contents.replace(
                "target->valid = true;",
                "unsigned char invalid=2;memcpy(&target->valid,&invalid,1);"
            )
        ),
    )
    .unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), "import records;async fn main(){try{let value=records.Packet(x:1,valid:false);let status=records.change(value,1);}catch errors{panic(\"catch resumed\");}finally{panic(\"finally resumed\");}}").unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "source_native_record_tests::source_async_invalid_record_writeback_exits_86_without_managed_resume", "--nocapture"]).env(CHILD, &root).output().unwrap();
    assert_eq!(output.status.code(), Some(86), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("native data contract violation (86)")
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("resumed"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_record_calls_share_buffers_write_back_status_and_preserve_argument_references() {
    let root = std::env::temp_dir().join(format!("rinzu-jit-writeback-{}", std::process::id()));
    writeback_fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(run.result, Ok(0), "{run:?}");
    assert_eq!(run.stdout, b"-7\n11\ntrue\n13\n18\n14\n7\n18\n9\n");
    assert!(run.native_calls.is_baseline(), "{run:?}");
    assert_eq!(run.native_calls.prepared, run.native_calls.released);
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_record_callbacks_catch_busy_and_discard_unentered_resources() {
    let root = std::env::temp_dir().join(format!("rinzu-jit-record-busy-{}", std::process::id()));
    writeback_fixture::create(&root);
    std::fs::write(
        root.join("app/src/main.rinzu"),
        include_str!("../tests/fixtures/native_record_writeback_sdk/busy.rinzu"),
    )
    .unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(run.result, Ok(0), "{run:?}");
    assert_eq!(run.stdout, b"-11\n12\n-12\n4\n-33\n4\n5\n64\n0\n78\n5\n");
    assert!(run.native_userdata.is_baseline(), "{run:?}");
    assert_eq!(run.heap_objects, 0);
    assert!(run.native_calls.is_baseline(), "{run:?}");
    assert_eq!(run.native_calls.prepared, run.native_calls.released);
    assert!(run.collection_count > 40);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_sdk_record_callbacks_preserve_private_capture_metadata_and_busy_errors() {
    let root = std::env::temp_dir().join(format!("rinzu-sdk-record-busy-{}", std::process::id()));
    writeback_fixture::create_sdk_callbacks(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    let descriptor = loaded
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.interface.package.package == "records")
        .unwrap();
    let sites = descriptor
        .interface
        .functions
        .iter()
        .flat_map(|function| &function.callback_sites)
        .filter(|site| site.capture.is_some())
        .collect::<Vec<_>>();
    assert!(!sites.is_empty());
    for site in sites {
        let capture = site.capture.as_ref().unwrap();
        assert_eq!(descriptor.interface.source_types[capture].kind, "state");
        for target in &site.targets {
            let function = descriptor
                .interface
                .functions
                .iter()
                .find(|function| function.identity == target.declaration)
                .unwrap();
            assert_eq!(function.parameter_requirements.first(), Some(capture));
        }
    }
    assert!(
        !descriptor
            .interface
            .public_bindings
            .values()
            .any(|identity| identity.starts_with("$rinzu_closure"))
    );
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(loaded),
    )
    .unwrap();
    assert_eq!(run.result, Ok(0), "{run:?}");
    assert_eq!(run.stdout, b"-11\n12\n-12\n4\n-33\n4\n5\n64\n0\n78\n5\n");
    assert_eq!(run.heap_objects, 0);
    assert!(
        run.native_calls.is_baseline() && run.native_userdata.is_baseline(),
        "{run:?}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_async_record_calls_write_back_aliases_and_recover_busy_callbacks() {
    for busy in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "rinzu-async-writeback-{}-{busy}",
            std::process::id()
        ));
        writeback_fixture::create(&root);
        if busy {
            std::fs::write(
                root.join("app/src/main.rinzu"),
                include_str!("../tests/fixtures/native_record_writeback_sdk/busy.rinzu"),
            )
            .unwrap();
        }
        let main = root.join("app/src/main.rinzu");
        let mut text = std::fs::read_to_string(&main)
            .unwrap()
            .replace("fn main()", "async fn main()");
        if !busy {
            text=text.replace("let result=execute();", "let result=execute();let workerValue=await spawn localRecordWorker();if workerValue!=10{panic(\"wrong Worker writeback\");}");
            text.push_str("fn localRecordWorker()->Int{try{let point=records.Packet(x:4,valid:false);let status=records.change(point,6);return point.x;}catch errors{panic(\"unexpected Worker writeback error\");return -1;}}");
        }
        std::fs::write(&main, text).unwrap();
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
                source_packages: Some(built.load_source_packages().unwrap()),
                capture_stdout: true,
                gc_stress: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            run.failure_details.is_none() && run.resources_returned,
            "{run:?}"
        );
        assert_eq!(
            run.stdout,
            if busy {
                b"-11\n12\n-12\n4\n-33\n4\n5\n64\n0\n78\n5\n".as_slice()
            } else {
                b"-7\n11\ntrue\n13\n18\n14\n7\n18\n9\n".as_slice()
            }
        );
        assert!(run.native_calls.is_baseline(), "{run:?}");
        assert!(run.heap.userdata.is_baseline(), "{run:?}");
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        if !busy {
            assert!(!run.worker_heaps.is_empty());
        }
        assert!(
            run.worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn supplant_record_snapshots_wait_for_entry_and_writeback_precedes_cancel_and_shutdown() {
    use rinzu_runtime::failure::FailureCause;
    let root = std::env::temp_dir().join(format!("rinzu-supplant-record-{}", std::process::id()));
    writeback_fixture::create(&root);
    for mode in ["normal", "queued-cancel", "active-cancel", "shutdown"] {
        let early = if mode == "queued-cancel" {
            "true"
        } else {
            "false"
        };
        std::fs::write(
            root.join("app/src/main.rinzu"),
            include_str!("../tests/fixtures/native_record_writeback_sdk/supplant.rinzu")
                .replace("EARLY", early),
        )
        .unwrap();
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
                source_packages: Some(built.load_source_packages().unwrap()),
                capture_stdout: true,
                gc_stress: true,
                cancel_after_polls: if mode == "active-cancel" {
                    Some(80)
                } else {
                    None
                },
                shutdown_after_polls: if mode == "shutdown" { Some(80) } else { None },
                ..Default::default()
            },
        )
        .unwrap();
        let cause = run
            .failure_details
            .as_ref()
            .map(|failure| failure.primary.cause);
        assert_eq!(
            cause,
            match mode {
                "normal" => None,
                "shutdown" => Some(FailureCause::Shutdown),
                _ => Some(FailureCause::Cancelled),
            },
            "{mode}: {run:?}"
        );
        assert_eq!(
            run.stdout,
            match mode {
                "normal" => b"-11\n32\n1\n".as_slice(),
                "queued-cancel" => b"1\n0\n".as_slice(),
                _ => b"32\n1\n".as_slice(),
            },
            "{mode}: {run:?}"
        );
        assert!(
            run.resources_returned && run.native_calls.is_baseline(),
            "{mode}: {run:?}"
        );
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0), "{mode}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn validated_record_descriptor_binds_exact_fields_to_the_shared_owner_schema() {
    let root = std::env::temp_dir().join(format!("rinzu-record-descriptor-{}", std::process::id()));
    for package in ["app", "records"] {
        std::fs::create_dir_all(root.join(package).join("src")).unwrap();
    }
    std::fs::write(root.join("records/rinzu.toml"),"[package]\nname=\"records\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("records/src/api.rinzu"),r#"
        native { record Packet { a:CInt8, b:CUInt64, c:CFloat, d:CBool }
          fn probe(value:Packet) -> CInt symbol "record_probe";
        }
        fn exercise() -> Int {
          let high: CUInt64 = 18446744073709551615;
          let value = Packet(a:CInt8(-128),b:high,c:CFloat(-0.0),d:true);
          var i=0; while i<40 { let temporary="moving record"; i=i+1; }
          if value.a != CInt8(-128) || value.b != high || value.c != CFloat(0.0) || value.d != true { panic("bad"); }
          let other = Packet(a:value.a,b:value.b,c:value.c,d:value.d);
          if value.a != other.a || value.b != other.b || value.c != other.c || value.d != other.d { panic("unequal"); }
          return i;
        }
        export Packet; export exercise;
    "#).unwrap();
    std::fs::write(root.join("records/api.h"),"#ifndef RECORD_PACKET_H\n#define RECORD_PACKET_H\n#include <stdint.h>\nstruct Packet {int8_t a; uint64_t b; float c; _Bool d;};\nint32_t record_probe(const struct Packet *value);\n#endif\n").unwrap();
    std::fs::write(
        root.join("records/api.c"),
        "#include \"api.h\"\nint32_t record_probe(const struct Packet *value) {return value->a;}\n",
    )
    .unwrap();
    std::fs::write(root.join("app/rinzu.toml"),"[package]\nname=\"record_app\"\nlanguage=\"0.2\"\n[dependencies]\nrecords={path=\"../records\"}\n").unwrap();
    std::fs::write(root.join("app/src/main.rinzu"),"import records; fn execute() -> Int {return records.exercise();} async fn main() {try {let value=execute();}catch unexpectedEntryErrors {panic(\"unexpectedEntryFailure\");}}").unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    let layouts = source_native_record_layouts(&artifact.ir, loaded.descriptors()).unwrap();
    assert_eq!(layouts.len(), 1);
    let identity = layouts.keys().next().unwrap();
    let layout = &layouts[identity];
    assert_eq!((layout.size(), layout.alignment()), (24, 8));
    assert_eq!(
        layout
            .fields()
            .iter()
            .map(|field| field.offset)
            .collect::<Vec<_>>(),
        [0, 8, 16, 20]
    );
    let descriptor_index = loaded
        .descriptors()
        .iter()
        .position(|descriptor| {
            descriptor
                .interface
                .types
                .iter()
                .any(|record| record.identity == *identity)
        })
        .unwrap();
    let record_index = loaded.descriptors()[descriptor_index]
        .interface
        .types
        .iter()
        .position(|record| record.identity == *identity)
        .unwrap();
    // Repeated catalog entries may describe a type used by several SDKs.
    // Matching layouts share one owner schema; contradictory copies reject.
    let mut repeated = loaded.descriptors().to_vec();
    repeated.push(repeated[descriptor_index].clone());
    let shared = source_native_record_layouts(&artifact.ir, &repeated).unwrap();
    assert_eq!(shared.len(), 1);
    assert_eq!(
        (shared[identity].size(), shared[identity].alignment()),
        (24, 8)
    );
    for mutation in 0..8 {
        let mut descriptors = loaded.descriptors().to_vec();
        let record = &mut descriptors[descriptor_index].interface.types[record_index];
        match mutation {
            0 => {
                record.fields[1].offset = "1".into();
            }
            1 => {
                record.fields[3].c_type = "int8_t".into();
            }
            2 => {
                record.fields.swap(0, 1);
            }
            3 => {
                record.size = "16".into();
            }
            4 => {
                record.canonical = "Other".into();
            }
            5 => {
                record.fields.pop();
            }
            6 => {
                descriptors[descriptor_index]
                    .interface
                    .types
                    .remove(record_index);
            }
            7 => {
                let mut duplicate = record.clone();
                duplicate.fields[0].c_type = "uint8_t".into();
                descriptors[descriptor_index]
                    .interface
                    .types
                    .push(duplicate);
            }
            _ => unreachable!(),
        }
        assert!(
            source_native_record_layouts(&artifact.ir, &descriptors).is_err(),
            "mutation {mutation}"
        );
    }
    let source = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            source_packages: Some(loaded),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        source.failure_details.is_none() && source.resources_returned,
        "{source:?}"
    );
    assert!(source.native_calls.is_baseline());
    let jit = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        BaselineOptions::default(),
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(40));
    assert_eq!(jit.heap_objects, 0);
    let equivalent = crate::aot::verify_equivalence(&root.join("app"), None).unwrap();
    assert_eq!(equivalent.outcome, "success");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_record_native_calls_preserve_fields_copies_roots_and_recoverable_errors() {
    use rinzu_runtime::failure::{ErrorKind, FailureCause};
    let root =
        std::env::temp_dir().join(format!("rinzu-source-record-calls-{}", std::process::id()));
    fixture::create(&root);
    let app = root.join("app");
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    let cleanup_signature = artifact
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
        .find(|import| import.identity == cleanup_signature.name)
        .unwrap();
    // SAFETY: validated ordinary Unit() binding, used after every Worker has returned.
    let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
    let normal = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            source_packages: Some(loaded),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        normal.failure_details.is_none() && normal.resources_returned,
        "{normal:?}"
    );
    assert!(normal.native_calls.is_baseline());
    assert_eq!(
        [
            normal.native_calls.prepared,
            normal.native_calls.returned,
            normal.native_calls.closed,
            normal.native_calls.settled,
            normal.native_calls.read,
            normal.native_calls.released
        ],
        [102; 6]
    );
    assert_eq!(normal.native_calls.callback_entries, 288);
    assert_eq!(normal.native_calls.copied_bytes, 1152);
    assert_eq!(normal.caught_errors.len(), 22);
    assert_eq!(
        normal
            .caught_errors
            .iter()
            .filter(
                |failure| failure.primary.cause == FailureCause::Error(ErrorKind::NullNativeValue)
            )
            .count(),
        11
    );
    assert_eq!(
        normal
            .caught_errors
            .iter()
            .filter(|failure| failure.primary.cause == FailureCause::Error(ErrorKind::Conversion))
            .count(),
        11
    );
    assert_eq!(normal.worker_heaps.len(), 10);
    assert!(
        normal
            .worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
    );
    assert_eq!((normal.heap.objects, normal.heap.roots), (0, 0));
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let cancelled = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                source_packages: Some(built.load_source_packages().unwrap()),
                cancel_worker_after_polls: Some(limit),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            cancelled.resources_returned && cancelled.native_calls.is_baseline(),
            "limit={limit}: {cancelled:?}"
        );
        assert_eq!(
            cancelled.native_calls.prepared,
            cancelled.native_calls.released
        );
        assert_eq!(
            cancelled.native_calls.returned,
            cancelled.native_calls.prepared
        );
        assert!(
            cancelled
                .worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
        assert_eq!((cancelled.heap.objects, cancelled.heap.roots), (0, 0));
    }
    // SAFETY: all native calls and physical Workers have finished using the private fixture page.
    unsafe {
        cleanup();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_record_calls_keep_native_copies_and_owner_roots_on_main_and_threads() {
    use rinzu_codegen::BaselineFailure;
    use rinzu_runtime::failure::{ErrorKind, FailureCause};
    let root = std::env::temp_dir().join(format!("rinzu-jit-record-calls-{}", std::process::id()));
    fixture::create(&root);
    std::fs::write(root.join("app/src/main.rinzu"),r#"
        import records;
        fn execute()->Int {let sample=records.exercise();return sample.caught;}
        fn nullFailure()->Int {let value=records.failure(0);return value.d;}
        fn boolFailure()->Int {let value=records.failure(2);return value.d;}
        fn leak()->Int {return records.leaks();}
        fn main(){try {let value=execute();if false{let missing=nullFailure();let invalid=boolFailure();let held=leak();}} catch unexpectedEntryErrors {panic("unexpectedEntryFailure");}}
    "#).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let options = BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let execute = |module: &rinzu_compiler::IrModule, packages| {
        run_baseline_jit_with_package_bindings(module, "execute", options, None, Some(packages))
            .unwrap()
    };
    let verify = |run: &rinzu_codegen::BaselineRun| {
        assert_eq!(run.result, Ok(2), "{run:?}");
        assert_eq!(run.heap_objects, 0);
        assert!(run.native_calls.is_baseline());
        assert_eq!(
            [
                run.native_calls.prepared,
                run.native_calls.returned,
                run.native_calls.closed,
                run.native_calls.settled,
                run.native_calls.read,
                run.native_calls.released
            ],
            [11; 6]
        );
        assert_eq!(run.native_calls.callback_entries, 32);
        assert_eq!(run.native_calls.copied_bytes, 128);
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(
            run.caught_errors[0].primary.cause,
            FailureCause::Error(ErrorKind::NullNativeValue)
        );
        assert_eq!(
            run.caught_errors[1].primary.cause,
            FailureCause::Error(ErrorKind::Conversion)
        );
        assert_eq!(run.published_code_versions, run.retired_code_versions);
        assert_eq!(run.rwx_mappings, 0);
        assert!(run.collection_count > 64);
    };
    let main = execute(&artifact.ir, built.load_source_packages().unwrap());
    verify(&main);
    let module = Arc::new(artifact.ir.clone());
    let artifacts = built
        .source_packages
        .iter()
        .map(|package| package.artifact.clone())
        .collect::<Vec<_>>();
    let workers = (0..8)
        .map(|_| {
            let module = Arc::clone(&module);
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
        verify(&worker.join().unwrap());
    }
    for (entry, failure) in [
        (
            "nullFailure",
            BaselineFailure::Error(ErrorKind::NullNativeValue),
        ),
        ("boolFailure", BaselineFailure::Conversion),
    ] {
        let run = run_baseline_jit_with_package_bindings(
            &artifact.ir,
            entry,
            options,
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        assert_eq!(run.result, Err(failure));
        assert_eq!(run.heap_objects, 0);
        assert!(run.native_calls.is_baseline());
        assert_eq!(run.native_calls.released, 1);
    }
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let run = run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "leak",
            BaselineOptions {
                cancel_after_polls: Some(limit),
                ..options
            },
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        assert_eq!(run.heap_objects, 0);
        assert!(run.native_calls.is_baseline());
        assert_eq!(run.native_calls.prepared, run.native_calls.returned);
        assert_eq!(run.native_calls.prepared, run.native_calls.released);
    }
    let loaded = built.load_source_packages().unwrap();
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
    // SAFETY: exact validated Unit() cleanup after all physical threads have returned.
    let cleanup: unsafe extern "C" fn() = unsafe { std::mem::transmute(import.glue_address()) };
    // SAFETY: no remaining fixture calls can read the private protected page.
    unsafe {
        cleanup();
    }
    std::fs::remove_dir_all(root).unwrap();
}
