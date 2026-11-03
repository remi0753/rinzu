#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{SourceAsyncOptions, SourceAsyncRun, run_source_async_report};
use rinzu_runtime::failure::{ErrorKind, FailureCause};
mod fixture {
    include!("../tests/fixtures/native_userdata_sdk/setup.rs");
}
mod context_fixture {
    include!("../tests/fixtures/native_context_records_sdk/setup.rs");
}

#[test]
fn supplant_context_results_settle_before_cancellation_and_shutdown() {
    let root = std::env::temp_dir().join(format!("rinzu-context-supplant-{}", std::process::id()));
    for mode in ["normal", "queued-cancel", "active-cancel", "shutdown"] {
        context_fixture::create_supplant(&root, mode == "queued-cancel");
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
                cancel_after_polls: (mode == "active-cancel").then_some(80),
                shutdown_after_polls: (mode == "shutdown").then_some(80),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            run.failure_details
                .as_ref()
                .map(|failure| failure.primary.cause),
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
                "normal" => b"10\n41\n33\n1\n".as_slice(),
                "queued-cancel" => b"20\n1\n0\n".as_slice(),
                _ => b"31\n33\n1\n".as_slice(),
            },
            "{mode}: {run:?}"
        );
        assert!(
            run.resources_returned && run.native_calls.is_baseline(),
            "{mode}: {run:?}"
        );
        assert!(run.heap.userdata.is_baseline());
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert_eq!(run.native_process.userdata, Default::default());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_results_restore_the_selected_input_and_release_after_null_errors() {
    let root = std::env::temp_dir().join(format!("rinzu-context-results-{}", std::process::id()));
    context_fixture::create_results(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(95), "{jit:?}");
    assert!(jit.native_calls.is_baseline() && jit.native_userdata.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    let source = run_source_async_report(
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
        source.failure_details.is_none()
            && source.resources_returned
            && source.native_calls.is_baseline(),
        "{source:?}"
    );
    assert_eq!(source.stdout, b"95\n");
    assert_eq!(source.caught_errors.len(), 2);
    assert!(source.caught_errors.iter().all(|failure| {
        failure.primary.cause == FailureCause::Error(ErrorKind::NullNativeValue)
    }));
    assert!(source.heap.userdata.is_baseline());
    assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
    assert_eq!(source.native_process.userdata, Default::default());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn context_records_retain_mutable_classes_native_references_and_closures_across_gc() {
    let root =
        std::env::temp_dir().join(format!("rinzu-context-references-{}", std::process::id()));
    context_fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let jit = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    assert_eq!(jit.result, Ok(25), "{jit:?}");
    assert!(jit.native_calls.is_baseline() && jit.native_userdata.is_baseline());
    assert_eq!(jit.heap_objects, 0);
    assert!(jit.collection_count > 40);
    let source = run_source_async_report(
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
        source.failure_details.is_none()
            && source.resources_returned
            && source.native_calls.is_baseline(),
        "{source:?}"
    );
    assert_eq!(source.stdout, b"25\n");
    assert!(source.heap.userdata.is_baseline());
    assert_eq!((source.heap.objects, source.heap.roots), (0, 0));
    let compiled = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(compiled.exit_code, 0, "{}", compiled.output);
    assert!(compiled.retired);
    crate::aot::compare_source_async_values_and_transport(&source, &compiled.output).unwrap();
    crate::aot::compare_source_async_failures(&source, &compiled.output).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ordinary_context_records_build_sdk_descriptors_and_callback_without_userdata_declarations() {
    let root =
        std::env::temp_dir().join(format!("rinzu-structural-context-{}", std::process::id()));
    for name in ["app", "context"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
    }
    std::fs::write(root.join("context/rinzu.toml"), "[package]\nname=\"context\"\nlanguage=\"0.2\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    std::fs::write(root.join("app/rinzu.toml"), "[package]\nname=\"context_app\"\nlanguage=\"0.2\"\n[dependencies]\ncontext={path=\"../context\"}\n").unwrap();
    std::fs::write(root.join("context/src/api.rinzu"), r#"
type Context={id:Int,text:String};
native{fn invoke(context:Context,handler:fn({text:String,id:Int})->Int)->Int symbol "context_invoke";}
fn read(context:Context)->Int fallback -1 {
    var index=0;while index<48{let moved=f"context-{index}";index=index+1;}
    return context.id;
}
fn exercise()->Int{return invoke({text="retained",id=42},read);}
export exercise;
"#).unwrap();
    std::fs::write(
        root.join("context/api.h"),
        "#include <stdint.h>\nint64_t context_invoke(void *,int64_t (*)(void *));\n",
    )
    .unwrap();
    std::fs::write(root.join("context/api.c"), "#include \"api.h\"\nint64_t context_invoke(void *value,int64_t (*handler)(void *)){return handler(value);}\n").unwrap();
    std::fs::write(root.join("app/src/main.rinzu"), "import context;fn execute()->Int{return context.exercise();}fn main(){let result=execute();}").unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let loaded = built.load_source_packages().unwrap();
    assert!(loaded.descriptors().iter().any(|descriptor| {
        descriptor
            .interface
            .source_types
            .keys()
            .any(|name| name.starts_with("$rinzu_record_"))
    }));
    let run = rinzu_codegen::run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        rinzu_codegen::BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
        None,
        Some(loaded),
    )
    .unwrap();
    assert_eq!(run.result, Ok(42), "{run:?}");
    assert!(
        run.native_calls.is_baseline() && run.native_userdata.is_baseline(),
        "{run:?}"
    );
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 40);
    std::fs::remove_dir_all(root).unwrap();
}

fn balanced(run: &SourceAsyncRun) {
    assert!(
        run.resources_returned && run.native_calls.is_baseline(),
        "{run:?}"
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.userdata.is_baseline());
    assert!(
        run.worker_heaps
            .iter()
            .all(|worker| worker.heap.objects == 0
                && worker.heap.roots == 0
                && worker.heap.userdata.is_baseline())
    );
    let calls = &run.native_calls;
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
    assert_eq!(calls.abandoned, 0);
    let tokens = run.heap.userdata.created
        + run
            .worker_heaps
            .iter()
            .map(|worker| worker.heap.userdata.created)
            .sum::<usize>();
    assert_eq!(tokens * 32, calls.callback_entries * 5);
    assert_eq!(calls.copied_bytes, calls.callback_entries * 4);
}

#[test]
fn source_userdata_actual_argument_tokens_survive_callback_gc_return_failure_and_cancellation() {
    let root = std::env::temp_dir().join(format!("rinzu-source-userdata-{}", std::process::id()));
    fixture::create(&root);
    let (manifest, artifact) = rinzu_compiler::check_package(&root.join("app")).unwrap();
    let built = crate::native_package::build_with_native_inputs(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )
    .unwrap();
    let normal = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            source_packages: Some(built.load_source_packages().unwrap()),
            record_transport_evidence: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(normal.failure_details.is_none(), "{normal:?}");
    balanced(&normal);
    assert_eq!(normal.native_calls.prepared, 119);
    assert_eq!(normal.native_calls.callback_entries, 3744);
    assert_eq!(normal.heap.userdata.created, 90);
    assert_eq!(normal.worker_heaps.len(), 11);
    assert_eq!(normal.caught_errors.len(), 3);
    assert!(
        normal
            .caught_errors
            .iter()
            .all(|failure| failure.primary.cause == FailureCause::Error(ErrorKind::Conversion))
    );
    assert_eq!(normal.caught_errors[1], normal.caught_errors[2]);
    assert!(normal.heap.moved_objects > 32);
    assert!(
        normal
            .worker_heaps
            .iter()
            .filter(|worker| worker.heap.userdata.created != 0)
            .all(|worker| worker.heap.moved_objects > 32)
    );
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        for worker in [false, true] {
            let cancelled = run_source_async_report(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    source_packages: Some(built.load_source_packages().unwrap()),
                    cancel_after_polls: (!worker).then_some(limit),
                    cancel_worker_after_polls: worker.then_some(limit),
                    record_transport_evidence: true,
                    ..Default::default()
                },
            )
            .unwrap();
            balanced(&cancelled);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_userdata_tokens_keep_owner_roots_until_actual_return_on_threads() {
    use rinzu_codegen::{
        BaselineFailure, BaselineOptions, BaselineRun, run_baseline_jit_with_package_bindings,
    };
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("rinzu-jit-userdata-{}", std::process::id()));
    fixture::create(&root);
    std::fs::write(
        root.join("app/src/main.rinzu"),
        r#"
import userdata;
fn execute()->Int {
    let value=userdata.exercise();let returned=userdata.returning();
    if value!=returned||returned!=userdata.original() {panic("lostData");}
    var caught=userdata.preEntryError();
    try {let failed=userdata.failed();}catch conversion {caught=caught+1;}
    return caught;
}
fn failure()->Int {return userdata.failed();}
fn main() {let value=execute();if false {let failed=failure();}}
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
    let options = BaselineOptions {
        collect_every_allocation: true,
        ..Default::default()
    };
    let balanced = |run: &BaselineRun| {
        assert!(
            run.native_calls.is_baseline() && run.native_userdata.is_baseline(),
            "{run:?}"
        );
        assert_eq!(run.heap_objects, 0);
        let calls = &run.native_calls;
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
        assert_eq!(calls.abandoned, 0);
        assert_eq!(run.native_userdata.created * 32, calls.callback_entries * 5);
        assert_eq!(calls.copied_bytes, calls.callback_entries * 4);
        assert_eq!(run.published_code_versions, run.retired_code_versions);
        assert_eq!(run.rwx_mappings, 0);
    };
    let verify = |run: &BaselineRun| {
        balanced(run);
        assert_eq!(run.result, Ok(2), "{run:?}");
        assert_eq!(run.native_calls.prepared, 17);
        assert_eq!(run.native_userdata.created, 75);
        assert_eq!(run.native_calls.callback_entries, 480);
        assert_eq!(run.caught_errors.len(), 2);
        assert!(
            run.caught_errors
                .iter()
                .all(|error| error.primary.cause == FailureCause::Error(ErrorKind::Conversion))
        );
        assert!(run.collection_count > 960);
    };
    let run = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "execute",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    verify(&run);
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
    let failed = run_baseline_jit_with_package_bindings(
        &artifact.ir,
        "failure",
        options,
        None,
        Some(built.load_source_packages().unwrap()),
    )
    .unwrap();
    balanced(&failed);
    assert_eq!(failed.result, Err(BaselineFailure::Conversion));
    assert_eq!(failed.native_calls.prepared, 2);
    assert_eq!(failed.native_userdata.created, 10);
    for limit in [1, 3, 5, 10, 20, 100, 1000] {
        let cancelled = run_baseline_jit_with_package_bindings(
            &artifact.ir,
            "execute",
            BaselineOptions {
                cancel_after_polls: Some(limit),
                ..options
            },
            None,
            Some(built.load_source_packages().unwrap()),
        )
        .unwrap();
        balanced(&cancelled);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn userdata_aot_matches_owner_arguments_gc_and_parked_activations() {
    let root = std::env::temp_dir().join(format!("rinzu-aot-userdata-{}", std::process::id()));
    fixture::create(&root);
    assert_eq!(
        crate::aot::verify_equivalence(&root.join("app"), None)
            .unwrap()
            .outcome,
        "success"
    );
    std::fs::remove_dir_all(root).unwrap();
}
