#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::*;

// Real process entries must handle logging and other ordinary Error effects.
// An unexpected Error still fails these probes through an uncatchable trap.
fn guard_main_errors(text: &str) -> String {
    let source =
        rinzu_compiler::SourceFile::new(rinzu_compiler::FileId(1), "guard-entry.rinzu", text);
    let syntax = rinzu_compiler::parse_source(&source).unwrap();
    let main = syntax
        .declarations
        .iter()
        .find_map(|declaration| match declaration {
            rinzu_compiler::ast::Declaration::Function(function) if function.name == "main" => {
                Some(function)
            }
            _ => None,
        })
        .unwrap();
    let range = main.body.range;
    let (start, end) = (range.start_byte as usize, range.end_byte as usize);
    format!(
        "{}try {{{}}} catch unexpectedEntryErrors {{panic(\"unexpectedEntryFailure\");}}{}",
        &text[..start + 1],
        &text[start + 1..end - 1],
        &text[end - 1..]
    )
}

#[test]
fn ordinary_async_finally_matches_backends_and_gc() {
    check_future_value_backends_inspect(
        "async_finally",
        include_str!("../../codegen/tests/fixtures/async-finally.rinzu"),
        b"body\ncleanup\npreserved-42\n",
        |_, _| {},
        true,
    );
}

#[test]
fn explicit_panic_matches_source_compiled_jit_and_aot_with_cleanup() {
    for (name, entry) in [
        (
            "panic_main",
            "async fn main(){let status=setProcessExitStatus(0);fail();}",
        ),
        ("panic_worker", "async fn main(){await spawn fail();}"),
        (
            "panic_discarded",
            "async fn child(){fail();}async fn main(){child();}",
        ),
    ] {
        let source = format!(
            r#"import rinzu.stdio;
fn line(text:String){{try{{rinzu.stdio.writeLine(text);}}catch errors{{}}}}
fn fail(){{try all{{panic("失敗🙂\u{{0}}tail");line("unreachable");}}catch errors{{line("caught");}}finally{{line("cleanup");}}}}
{entry}"#
        );
        check_future_backends_outcome(
            name,
            &source,
            b"cleanup\n",
            |_, output| {
                assert!(output.contains("RINZU_FAILURE=panic"), "{output}");
                // The embedded compiled JIT returns its structured report;
                // native stderr is written directly to its host. A standalone
                // AOT child has both streams captured by this test.
                if output.contains("\"producer\":\"rinzu-aot\"") {
                    assert!(output.contains("panic: 失敗🙂\0tail"), "{output}");
                }
            },
            true,
            Some(rinzu_runtime::failure::FailureCause::Panic),
        );
    }
}

#[test]
fn sdk_direct_results_and_error_payloads_match_backends() {
    check_future_value_backends_inspect(
        "sdk_direct_errors",
        include_str!("../../codegen/tests/fixtures/sdk-direct-errors.rinzu"),
        b"610\n-7104\n-7504\n-7304\n0\n0\n0\n0\n0\n0\n",
        |source, _| {
            assert!(source.native_calls.is_baseline());
        },
        true,
    );
}

#[test]
fn fallible_data_intrinsics_match_source_compiled_jit_and_aot() {
    check_future_value_backends_inspect("fallible_data",
        include_str!("../../codegen/tests/fixtures/fallible-data.rinzu"),
        b"42\nhello\n42\n42\n42\n42\n42\n0\n1\n201\n202\n200\n301\n300\n300\n300\n100\n-7\n101\n2\n", |_,_|{}, true);
}

#[test]
fn async_rejection_and_discarded_traps_match_backends() {
    use rinzu_runtime::failure::FailureCause;
    for (name, text, cause) in [
        (
            "discarded_trap",
            "async fn bad(){let invalid=1/0;} async fn main(){bad();}",
            FailureCause::ArithmeticTrap,
        ),
        (
            "spawn_rejection",
            "fn echo(value:Int)->Int{return value;} async fn main(){let a=spawn echo(1);let b=spawn echo(2);let c=spawn echo(3);let d=spawn echo(4);let e=spawn echo(5);let f=spawn echo(6);let g=spawn echo(7);let h=spawn echo(8);let denied=spawn echo(9);panic(\"spawn admission unexpectedly succeeded\");}",
            FailureCause::Overloaded,
        ),
    ] {
        check_future_backends_outcome(
            name,
            text,
            b"",
            |source, _| {
                if cause == FailureCause::Overloaded {
                    assert_eq!(source.future_admissions.len(), 8);
                }
            },
            true,
            Some(cause),
        );
    }
}

#[test]
fn native_error_entries_match_source_compiled_jit_and_aot_with_gc() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-native-error-boundaries-{}",
        std::process::id()
    ));
    let app = root.join("app");
    let sdk = root.join("handlers");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(sdk.join("src")).unwrap();
    fs::write(app.join("rinzu.toml"),"[package]\nname=\"native_error_boundaries\"\nlanguage=\"0.5\"\n[dependencies]\nhandlers={path=\"../handlers\"}\n").unwrap();
    fs::write(sdk.join("rinzu.toml"),"[package]\nname=\"handlers\"\nlanguage=\"0.5\"\nkind=\"sdk\"\nentry=\"src/api.rinzu\"\n[native]\nsources=[\"api.c\"]\nheaders=[\"api.h\"]\n").unwrap();
    fs::write(
        sdk.join("src/api.rinzu"),
        include_str!("../tests/fixtures/error_native_sdk/api.rinzu"),
    )
    .unwrap();
    fs::write(
        sdk.join("api.h"),
        include_str!("../tests/fixtures/error_native_sdk/api.h"),
    )
    .unwrap();
    fs::write(
        sdk.join("api.c"),
        include_str!("../tests/fixtures/error_native_sdk/api.c"),
    )
    .unwrap();
    fs::write(
        app.join("src/main.rinzu"),
        include_str!("../tests/fixtures/error_native_sdk/main.rinzu"),
    )
    .unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&app).unwrap();
    let built = build_package(&app, Some(root.join("program"))).unwrap();
    let reference = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            record_transport_evidence: true,
            package_root: manifest.root.clone(),
            source_packages: Some(
                crate::source_package_bundle::load(
                    &with_suffix(&built.executable, ".source-packages").join("manifest.json"),
                )
                .unwrap(),
            ),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(reference.failure_details.is_none(), "{reference:#?}");
    assert!(reference.resources_returned && reference.native_calls.is_baseline());
    assert_eq!((reference.heap.objects, reference.heap.roots), (0, 0));
    assert_eq!(reference.native_calls.prepared, 29);
    assert_eq!(reference.native_calls.callback_entries, 27);
    let expected = b"427\n-17\n-19\n4\n-23\n-23\n";
    assert_eq!(reference.stdout, expected);
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    assert!(jit.retired);
    compare_source_async_values_and_transport(&reference, &jit.output).unwrap();
    compare_source_async_failures(&reference, &jit.output).unwrap();
    let output = Command::new(&built.executable)
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
    let sanitized = super::callback_tests::sanitized(&built);
    let output = Command::new(sanitized)
        .env("RINZU_GC_STRESS", "1")
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn async_start_errors_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_start_errors_backends",
        include_str!("../../codegen/tests/fixtures/future-start-errors.rinzu"),
        b"timer:0\npath:0\nargument:0\nbody:1\n1\n2\n",
    );
}

#[test]
fn future_gc_matches_source_compiled_jit_and_aot() {
    check_future_value_backends_inspect(
        "future_gc_backends",
        include_str!("../../codegen/tests/fixtures/future-gc.rinzu"),
        b"4096\n64\nretained through finally\nretained for queued observer\n",
        |reference, native| {
            assert!(reference.peak_future_records < 256);
            assert!(reference.gc_retired_futures >= 4096);
            assert_eq!(reference.remaining_future_records, 0);
            let counts = parse_line(native, "RINZU_FUTURE_GC=")
                .unwrap()
                .split(',')
                .map(|n| n.parse::<usize>().unwrap())
                .collect::<Vec<_>>();
            eprintln!(
                "Future GC: source peak={}, retired={}; native={counts:?}",
                reference.peak_future_records, reference.gc_retired_futures
            );
            assert!(counts[0] <= 1152, "{counts:?}");
            assert!(counts[1] >= 4096, "{counts:?}");
            assert_eq!(counts[2], 0);
        },
        true,
    );
}

#[test]
fn independent_task_lifetimes_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_lifetime_backends",
        include_str!("../../codegen/tests/fixtures/future-lifetime.rinzu"),
        b"17\nparent only\nmain complete\nlate:1\nlate:2\n",
    );
    check_future_value_backends(
        "future_sync_lifetime_backends",
        include_str!("../../codegen/tests/fixtures/future-sync-lifetime.rinzu"),
        b"sync main complete\nretained after sync main\n",
    );
}

#[test]
fn future_metadata_gc_without_heap_allocations_matches_all_backends() {
    check_future_value_backends_inspect(
        "future_gc_unit_backends",
        include_str!("../../codegen/tests/fixtures/future-gc-unit.rinzu"),
        b"",
        |reference, native| {
            assert_eq!(reference.heap.peak_objects, 0);
            assert!(reference.peak_future_records <= 1152);
            assert!(reference.gc_retired_futures >= 4096);
            assert_eq!(reference.remaining_future_records, 0);
            let counts = parse_line(native, "RINZU_FUTURE_GC=")
                .unwrap()
                .split(',')
                .map(|n| n.parse::<usize>().unwrap())
                .collect::<Vec<_>>();
            assert!(counts[0] <= 1152, "{counts:?}");
            assert!(counts[1] >= 4096, "{counts:?}");
            assert_eq!(counts[2], 0);
        },
        false,
    );
}

#[test]
fn queued_async_bodies_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_queued_backends",
        include_str!("../../codegen/tests/fixtures/future-queued.rinzu"),
        "1\n界\0🙂\n0\n".as_bytes(),
    );
}

#[test]
fn main_error_boundaries_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "error_main_boundaries_backends",
        include_str!("../../codegen/tests/fixtures/error-main-boundaries.rinzu"),
        b"383\n389\n397\n",
    );
}

#[test]
fn worker_local_errors_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "error_worker_boundaries_backends",
        include_str!("../../codegen/tests/fixtures/error-worker-boundaries.rinzu"),
        b"347\n702\n726\n",
    );
}

#[test]
fn future_observer_collections_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_observer_collections_backends",
        include_str!("../../codegen/tests/fixtures/future-observer-collections.rinzu"),
        b"331\n337\n343\n",
    );
}

#[test]
fn future_receiver_coverage_matches_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_receiver_coverage_backends",
        include_str!("../../codegen/tests/fixtures/future-receiver-coverage.rinzu"),
        b"317\n319\n",
    );
}

#[test]
fn future_observer_controls_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_observer_control_backends",
        include_str!("../../codegen/tests/fixtures/future-observer-controls.rinzu"),
        b"307\n311\n307\n",
    );
}

#[test]
fn future_capture_matches_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_capture_backends",
        include_str!("../../codegen/tests/fixtures/future-capture.rinzu"),
        b"277\n281\n283\n293\n277\n",
    );
}

#[test]
fn future_discard_matches_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_discard_backends",
        include_str!("../../codegen/tests/fixtures/future-discard.rinzu"),
        b"241\n251\n257\n263\n269\n",
    );
}

#[test]
fn future_async_fields_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_async_field_backends",
        include_str!("../../codegen/tests/fixtures/future-async-fields.rinzu"),
        b"211\n223\n227\n229\n229\n233\n239\n",
    );
}

#[test]
fn future_field_observers_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_field_observer_backends",
        include_str!("../../codegen/tests/fixtures/future-field-observers.rinzu"),
        b"193\n197\n199\n203\n204\n",
    );
}

#[test]
fn future_field_calls_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_field_call_backends",
        include_str!("../../codegen/tests/fixtures/future-field-calls.rinzu"),
        b"163\n167\n173\n181\n181\n",
    );
}

#[test]
fn future_mutable_objects_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_mutable_object_backends",
        include_str!("../../codegen/tests/fixtures/future-mutable-objects.rinzu"),
        b"131\n137\n149\n157\n137\n",
    );
}

#[test]
fn future_growing_maps_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_growing_map_backends",
        include_str!("../../codegen/tests/fixtures/future-growing-map.rinzu"),
        b"0\n1\n2\n0\n1\n1\n",
    );
}

#[test]
fn future_growing_lists_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_growing_list_backends",
        include_str!("../../codegen/tests/fixtures/future-growing-list.rinzu"),
        b"0\n1\n2\n0\n1\n1\n",
    );
}

#[test]
fn future_map_observations_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_map_observation_backends",
        include_str!("../../codegen/tests/fixtures/future-map-observations.rinzu"),
        b"3\n0\n113\n1\n127\n",
    );
}

#[test]
fn future_pending_arguments_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_pending_argument_backends",
        include_str!("../../codegen/tests/fixtures/future-pending-arguments.rinzu"),
        b"101\n103\n101\n101\n",
    );
}

#[test]
fn future_list_observations_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_list_observation_backends",
        include_str!("../../codegen/tests/fixtures/future-list-observations.rinzu"),
        b"3\n0\n2\n",
    );
}

#[test]
fn future_observations_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_observation_backends",
        include_str!("../../codegen/tests/fixtures/future-observations.rinzu"),
        b"71\n73\n79\n83\n89\n97\n7\n",
    );
}

#[test]
fn normal_error_origins_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "normal_error_origin_backends",
        include_str!("../../codegen/tests/fixtures/normal-error-origins.rinzu"),
        b"73\n79\n83\n",
    );
}

#[test]
fn callable_effects_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "callable_effect_backends",
        include_str!("../../codegen/tests/fixtures/callable-error-effects.rinzu"),
        b"41\n37\n47\n37\n43\n",
    );
}

#[test]
fn future_factories_and_immutable_values_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_value_backends",
        include_str!("../../codegen/tests/fixtures/future-error-values.rinzu"),
        b"11\n13\n17\n23\n29\n",
    );
}
#[test]
fn future_parameter_effects_match_source_compiled_jit_and_aot() {
    check_future_value_backends(
        "future_parameter_backends",
        include_str!("../../codegen/tests/fixtures/future-parameter-errors.rinzu"),
        b"17\n29\n31\n17\n",
    );
}
fn check_future_value_backends(name: &str, text: &str, expected: &[u8]) {
    check_future_value_backends_inspect(name, text, expected, |_, _| {}, false);
}

fn check_future_value_backends_inspect(
    name: &str,
    text: &str,
    expected: &[u8],
    inspect: impl Fn(&rinzu_codegen::SourceAsyncRun, &str),
    sanitize: bool,
) {
    check_future_backends_outcome(name, text, expected, inspect, sanitize, None);
}

fn check_future_backends_outcome(
    name: &str,
    text: &str,
    expected: &[u8],
    inspect: impl Fn(&rinzu_codegen::SourceAsyncRun, &str),
    sanitize: bool,
    failure: Option<rinzu_runtime::failure::FailureCause>,
) {
    let root = std::env::temp_dir().join(format!("rinzu-{name}-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        format!("[package]\nname=\"{name}\"\nlanguage=\"0.5\"\n"),
    )
    .unwrap();
    fs::write(root.join("src/main.rinzu"), text).unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let built = build_package(&root, Some(root.join("build/program"))).unwrap();
    let source_manifest = with_suffix(&built.executable, ".source-packages").join("manifest.json");
    let reference = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            record_transport_evidence: true,
            package_root: manifest.root.clone(),
            source_packages: source_manifest
                .exists()
                .then(|| crate::source_package_bundle::load(&source_manifest).unwrap()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        reference
            .failure_details
            .as_ref()
            .map(|failure| failure.primary.cause),
        failure,
        "{reference:#?}"
    );
    assert!(reference.resources_returned);
    assert_eq!(reference.stdout, expected);
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(
        jit.exit_code,
        i32::from(failure.is_some()),
        "{}",
        jit.output
    );
    assert!(jit.retired);
    compare_source_async_values_and_transport(&reference, &jit.output).unwrap();
    compare_source_async_failures(&reference, &jit.output).unwrap();
    inspect(&reference, &jit.output);
    let output = Command::new(&built.executable)
        .env("RINZU_GC_STRESS", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success() == failure.is_none(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, expected);
    inspect(&reference, &String::from_utf8_lossy(&output.stderr));
    if sanitize {
        let executable = super::callback_tests::sanitized(&built);
        let output = Command::new(executable)
            .env("RINZU_GC_STRESS", "1")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.success(), failure.is_none(), "{stderr}");
        assert!(
            !stderr.contains("ERROR: AddressSanitizer") && !stderr.contains("runtime error:"),
            "{stderr}"
        );
        assert_eq!(output.stdout, expected);
        inspect(&reference, &stderr);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_error_traces_match_rust_compiled_jit_and_source_free_aot() {
    use serde_json::{Value, json};
    let root =
        std::env::temp_dir().join(format!("rinzu-error-trace-backends-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"trace_backends\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    for (index, text, asynchronous) in [
        (
            0,
            r#"
error A{} error B{}
fn leaf(problem:A){let text="界🙂";throw problem;}
fn relay(problem:A){try{leaf(problem);}catch[first:A]{throw first;}}
fn cleanup(){throw B(message:"cleanup");}
fn convert(){let invalid=CInt8(300);}
fn main(){
    let problem=A(message:"original");
    try{try{relay(problem);}finally{cleanup();}}catch[A,B]{}
    try{throw problem;}catch[A]{}
    try{convert();}catch errors{}
}
"#,
            false,
        ),
        (
            1,
            r#"
import rinzu.timer;
error A{} error B{}
async fn timer(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }

fn leaf(){let text="界🙂";throw A(message:"original");}
fn cleanup(){throw B(message:"cleanup");}
async fn work(){await timer(1);leaf();}
async fn main(){
    let pending=work();
    try{await pending;}catch errors{}
    try{await pending;}catch errors{}
    try{try{await pending;}finally{cleanup();}}catch errors{}
}
"#,
            true,
        ),
        (
            2,
            r#"
fn leaf(){let invalid=CInt8(300);}
fn work()->Int{try{leaf();}catch errors{}return 7;}
async fn main(){let pending=spawn work();try{let ignored=await pending;}catch errors{}}
"#,
            true,
        ),
        (
            3,
            include_str!("../../codegen/tests/fixtures/scalar-source-locations.rinzu"),
            false,
        ),
    ] {
        let path = root.join("src/main.rinzu");
        fs::write(&path, text).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let source = rinzu_compiler::SourceFile::new(rinzu_compiler::FileId(0), &path, text);
        let files = rinzu_codegen::resource_source_files(&artifact.ir);
        let expected = |failures: &[rinzu_runtime::failure::Failure]| {
            Value::Array(failures.iter().map(|failure| {
                Value::Array((0..=failure.cleanup.len()).map(|index| {
                    failure.traces.get(&index).map_or(Value::Null,|trace| {
                        Value::Array(trace.frames.iter().map(|frame| {
                            json!({"function":frame.function.as_ref(),"source":frame.source.map(|at| {
                                let (line,column)=source.line_column(at.start_byte as usize);
                                json!({"file":at.file,"startByte":at.start_byte,"endByte":at.end_byte,"name":files[&at.file],"line":line,"column":column})
                            })})
                        }).collect())
                    })
                }).collect())
            }).collect())
        };
        let caught = if asynchronous {
            let run = rinzu_codegen::run_source_async_report(
                &artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    capture_error_trace: true,
                    gc_stress: true,
                    package_root: manifest.root.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(run.failure_details.is_none(), "{run:#?}");
            assert!(run.resources_returned);
            run.caught_errors
        } else {
            let run = rinzu_codegen::run_baseline_jit_with_options(
                &artifact.ir,
                "main",
                BaselineOptions {
                    capture_error_trace: true,
                    collect_every_allocation: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(run.result.is_ok(), "{run:#?}");
            assert_eq!(run.heap_objects, 0);
            run.caught_errors
        };
        assert!(!caught.is_empty());
        let expected = json!({"current":[],"caught":expected(&caught)});
        if asynchronous {
            crate::compiled_async::configure_error_trace_probe();
            let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
            assert_eq!(jit.exit_code, 0, "{}", jit.output);
            assert!(jit.retired);
            let traces = parse_line(&jit.output, "RINZU_ERROR_TRACES=").unwrap();
            assert_eq!(serde_json::from_str::<Value>(traces).unwrap(), expected);
        }
        let built = build_package(&root, Some(root.join(format!("build/trace{index}")))).unwrap();
        fs::remove_file(&path).unwrap();
        for enabled in [false, true] {
            let mut command = Command::new(&built.executable);
            command
                .env("RINZU_GC_STRESS", "1")
                .env_remove("RINZU_ERROR_TRACE");
            if enabled {
                command.env("RINZU_ERROR_TRACE", "1");
            }
            let output = command.output().unwrap();
            let report = String::from_utf8(output.stderr).unwrap();
            assert!(output.status.success(), "{report}");
            if enabled {
                let traces = parse_line(&report, "RINZU_ERROR_TRACES=").unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(traces).unwrap(),
                    expected,
                    "{report}"
                );
            } else {
                assert!(!report.contains("RINZU_ERROR_TRACES="), "{report}");
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dedicated_errors_match_rust_jit_and_aot_with_payload_gc_and_repeated_await() {
    let root = std::env::temp_dir().join(format!("rinzu-error-backends-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"error_backends\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    let synchronous = r#"import rinzu.List;

        import rinzu.stdio;
        class Document { value: Int }
        error A { payload: Document }
        error B { payload: Int }
        error Optional<T> { payload?: T }
        fn fail(document: Document) { throw A(message: "a", payload: document); }
        fn relay(document: Document) {
            try { fail(document); }
            catch [error: A] { document.value=19; throw error; }
        }
        fn inspect(problem:Error,document:Document)->Int {
            if problem is A && problem.payload.value>0 {document.value=47;return problem.payload.value;}
            return -1;
        }
        fn chosen()->Int {
            try all {throw B(message:"choice",payload:7);return 19;}
            catch errors {rinzu.stdio.writeLine(rinzu.List.length(errors));}
            finally {rinzu.stdio.writeLine("all-finally");}
            return 0;
        }
        fn main() {
            let document=Document(value: 3);
            try { try {relay(document);} finally {throw B(message:"b",payload:7);} }
            catch [first: A, second: B, ...rest] {
                document.value=31;
                rinzu.stdio.writeLine(first.payload.value);
                first.payload.value=43;
                rinzu.stdio.writeLine(document.value);
                rinzu.stdio.writeLine(second.payload);
                rinzu.stdio.writeLine(rinzu.List.length(rest));
            }
            let optional=Optional<Int>(message:"optional",payload:23);
            rinzu.stdio.writeLine(optional.name);
            rinzu.stdio.writeLine(optional.payload);
            rinzu.stdio.writeLine(inspect(A(message:"check",payload:document),document));
            try {try {fail(document);}finally {throw B(message:"list",payload:5);}}
            catch errors {rinzu.stdio.writeLine(rinzu.List.length(errors));}
            try all {throw A(message:"first",payload:document);throw B(message:"second",payload:23);}
            catch [first:A,second:B] {rinzu.stdio.writeLine(first.payload.value);rinzu.stdio.writeLine(second.payload);}
            rinzu.stdio.writeLine(chosen());
        }
    "#;
    let asynchronous = r#"import rinzu.List;

        import rinzu.stdio;
        error Failed { payload: Int }
        async fn work() -> Int { throw Failed(message:"failed",payload:11); }
        async fn main() {
            let pending=work();
            try {let firstValue=await pending;}
            catch [error: Failed] {rinzu.stdio.writeLine(error.payload);}
            try {let value=await pending;}
            catch [error: Failed] {rinzu.stdio.writeLine(error.payload);}
            try all {let one=await pending;let two=await pending;}
            catch errors {rinzu.stdio.writeLine(rinzu.List.length(errors));}
            try {try all {} finally {let cleanupValue=await pending;}}
            catch [error:Failed] {rinzu.stdio.writeLine("cleanup outside");}
        }
    "#;
    let shutdown_cleanup = r#"
        import rinzu.stdio;
        import rinzu.timer;
async fn timer(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }

        async fn grandchild() { await timer(1); rinzu.stdio.writeLine("GRANDCHILD"); }
        async fn child() { await grandchild(); rinzu.stdio.writeLine("CHILD"); }
        async fn main() {
            try all { let requested=requestShutdown(); }
            finally { let pending=child(); await pending; }
        }
    "#;
    for (index, text, expected) in [
        (
            0,
            synchronous,
            &b"31\n43\n7\n0\nOptional\nOption<Int>.Some(value: 23)\n47\n2\n47\n23\n1\nall-finally\n19\n"[..],
        ),
        (1, asynchronous, &b"11\n11\n1\ncleanup outside\n"[..]),
        (2, shutdown_cleanup, &b"GRANDCHILD\nCHILD\n"[..]),
    ] {
        fs::write(root.join("src/main.rinzu"), guard_main_errors(text)).unwrap();
        verify_equivalence_with_options(
            &root,
            BaselineOptions {
                collect_every_allocation: true,
                ..Default::default()
            },
        )
        .unwrap();
        let built = build_package(&root, Some(root.join(format!("build/program{index}")))).unwrap();
        let output = Command::new(&built.executable)
            .env("RINZU_GC_STRESS", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn compiled_async_jit_observes_managed_future_errors() {
    let root = std::env::temp_dir().join(format!("rinzu-error-async-jit-{}", std::process::id()));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"error_jit\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    fs::write(
        root.join("src/main.rinzu"),
        guard_main_errors(r#"import rinzu.List;

        import rinzu.stdio;
        error Failed { payload: Int }
        async fn work() -> Int { throw Failed(message:"failed",payload:11); }
        fn inspect(problem:Error){if problem is Failed {rinzu.stdio.writeLine(problem.payload);}else{rinzu.stdio.writeLine("wrong type");}}
        async fn main() {
            let pending=work();
            try {let first=await pending;}
            catch [error: Failed] {inspect(error);}
            try all {let one=await pending;let two=await pending;}
            catch errors {rinzu.stdio.writeLine(rinzu.List.length(errors));}
            try {let second=await pending;}
            catch [error: Failed] {inspect(error);}
            try {try all {} finally {let cleanupValue=await pending;}}
            catch [error:Failed] {rinzu.stdio.writeLine("cleanup outside");}
        }
    "#),
    )
    .unwrap();
    let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
    let reference = rinzu_codegen::run_source_async_report(
        &artifact.ir,
        "main",
        rinzu_codegen::SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            record_transport_evidence: true,
            package_root: manifest.root.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(reference.stdout, b"11\n1\n11\ncleanup outside\n");
    let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
    assert_eq!(jit.exit_code, 0, "{}", jit.output);
    assert!(jit.retired);
    compare_source_async_values_and_transport(&reference, &jit.output).unwrap();
    compare_source_async_failures(&reference, &jit.output).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn error_source_locations_match_backends_and_aot_runs_without_the_source_file() {
    let root = std::env::temp_dir().join(format!(
        "rinzu-error-source-backends-{}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("rinzu.toml"),
        "[package]\nname=\"error_source_backends\"\nlanguage=\"0.5\"\n",
    )
    .unwrap();
    let report = r#"
        import rinzu.stdio;
        error Failed {payload:Int}
        fn report(problem:Error){switch problem.source {
        case Option.None:{rinzu.stdio.writeLine("none");}
        case Option.Some(at):{rinzu.stdio.writeLine(at.file);rinzu.stdio.writeLine(at.line);rinzu.stdio.writeLine(at.column);}
        }}
    "#;
    let synchronous = format!(
        "{report}\r\n{}",
        r#"
        fn raise(problem:Failed){let text="界🙂"; throw problem;}
        fn relay(problem:Failed){try{raise(problem);}catch[first:Failed]{throw first;}}
        fn main(){
            let problem=Failed(message:"first",payload:17);
            report(problem);
            try{relay(problem);}catch[first:Failed]{report(first);}
            try{let value=CInt8(300);}catch[first:Error,...rest]{report(first);}
            let legacy=error(7,"legacy");
            try{throw legacy;}catch[first:Error,...rest]{report(first);}
        }
    "#
    );
    let asynchronous = format!(
        "{report}\r\n{}",
        r#"
        async fn work()->Int{throw Failed(message:"async",payload:23);}
        async fn main(){
            let pending=work();
            try{let firstValue=await pending;}catch[first:Failed]{report(first);}
            try{let secondValue=await pending;}catch[first:Failed]{report(first);}
        }
    "#
    );
    for (index, text, tokens) in [
        (
            0,
            synchronous,
            vec!["throw problem", "CInt8(300)", "throw legacy"],
        ),
        (1, asynchronous, vec!["throw Failed", "throw Failed"]),
    ] {
        let text = guard_main_errors(&text);
        let path = root.join("src/main.rinzu");
        fs::write(&path, &text).unwrap();
        let (manifest, artifact) = rinzu_compiler::check_package(&root).unwrap();
        let main = artifact
            .ir
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        let name = &artifact.ir.source_files[&main.range.file_id.0];
        let file = rinzu_compiler::SourceFile::new(main.range.file_id, &path, &text);
        let mut expected = if index == 0 {
            "none\n".to_owned()
        } else {
            String::new()
        };
        for token in tokens {
            let (line, column) = file.line_column(text.find(token).unwrap());
            expected.push_str(&format!("{name}\n{line}\n{column}\n"));
        }
        verify_equivalence_with_options(
            &root,
            BaselineOptions {
                collect_every_allocation: true,
                ..Default::default()
            },
        )
        .unwrap();
        if index == 1 {
            let reference = rinzu_codegen::run_source_async_report(
                &artifact.ir,
                "main",
                rinzu_codegen::SourceAsyncOptions {
                    capture_stdout: true,
                    gc_stress: true,
                    record_transport_evidence: true,
                    package_root: manifest.root.clone(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(reference.stdout, expected.as_bytes());
            let jit = crate::compiled_async::run(&manifest, &artifact, None, None, true).unwrap();
            assert_eq!(jit.exit_code, 0, "{}", jit.output);
            assert!(jit.retired);
            compare_source_async_values_and_transport(&reference, &jit.output).unwrap();
            compare_source_async_failures(&reference, &jit.output).unwrap();
        }
        let built = build_package(&root, Some(root.join(format!("build/source{index}")))).unwrap();
        fs::remove_file(path).unwrap();
        let output = Command::new(&built.executable)
            .env("RINZU_GC_STRESS", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected.as_bytes());
    }
    fs::remove_dir_all(root).unwrap();
}
