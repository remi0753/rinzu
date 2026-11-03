#[path = "../test-support/library.rs"]
mod test_library;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::emit_aot_object;
use rinzu_codegen::run_jit_spike;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_codegen::{
    BaselineFailure, BaselineOptions, run_baseline_jit, run_baseline_jit_with_options,
};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use rinzu_compiler::{FileId, SourceFile};
use test_library::compile_library;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn captured_stdout_survives_moving_gc_and_keeps_exact_bytes() {
    let artifact = compile_library(&SourceFile::new(
        FileId(195),
        "stdio.rinzu",
        r#"
import rinzu.stdio;
fn copy(text: String) -> String { let moved = ["allocation", "roots"]; return text; }
fn main() { let text = "日本語\u{0}🙂"; rinzu.stdio.write(copy(text)); rinzu.stdio.writeLine(""); }
"#,
    ))
    .unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            collect_every_allocation: true,
            capture_stdout: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.stdout, "日本語\0🙂\n".as_bytes());
    assert!(run.collection_count > 0);
    assert_eq!(run.heap_objects, 0);
    assert_eq!(run.published_code_versions, run.retired_code_versions);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn formatted_strings_keep_parts_and_return_values_through_moving_gc() {
    let source = SourceFile::new(
        FileId(1),
        "formatted.rinzu",
        r#"
import rinzu.stdio;
class Item { const label: String, const value: CSize }
fn item() -> Item { return Item(label: "日本語\u{0}", value: 18446744073709551615); }
fn text() -> String { return f"前\{\}:{item()}:{["a", "b"]}:{f"内{true}"}"; }
fn probe() -> Int { let result = text(); rinzu.stdio.writeLine(result); return Int(utf8ByteLength(result)); }
fn main() {}
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let expected =
        "前{}:Item(label: \"日本語\\u{00}\", value: 18446744073709551615):[\"a\", \"b\"]:内true\n";
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "probe",
        BaselineOptions {
            collect_every_allocation: true,
            capture_stdout: true,
            ..BaselineOptions::default()
        },
    )
    .unwrap();
    assert_eq!(run.stdout, expected.as_bytes());
    assert_eq!(run.result, Ok(expected.len() as i64 - 1));
    assert!(run.collection_count > 0);
    assert_eq!(run.heap_objects, 0);
    assert_eq!(run.published_code_versions, run.retired_code_versions);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn compiled_return_and_loop_exits_run_finally_with_rooted_recursive_results() {
    let source = SourceFile::new(
        FileId(193),
        "compiled-return.rinzu",
        r#"
fn returning() -> String {
    try { try { return "retained"; }
        finally { try { let badInner=CInt8(300); } catch [inner: Error, ...innerRest] { let copiedInner=inner.message; } }
    } finally { try { let badOuter=CInt8(400); } catch [outer: Error, ...outerRest] { let copiedOuter=outer.message; } }
}
fn recursive(depth:Int) -> String {
    try {if depth<4 {return recursive(depth+1);}else{return "retained";}}
    finally {let moved=["moving","cleanup","roots"];}
}
fn unpublished() -> String {try {return "unpublished";}finally {let badCleanup=CInt8(200);}}
fn execute() -> Int {
    let ordinary=returning();let nested=recursive(0);
    if ordinary!="retained"||nested!=ordinary {panic("lostRoot");}
    try {let ignored=unpublished();}catch [cleanupError: Error, ...cleanupErrorRest] {let copied=cleanupError.message;}
    var count=0;
    while count<3 {try {continue;}finally {count=count+1;}}
    while true {try {break;}finally {count=count+1;}}
    return count;
}
fn main() {let result=execute();}
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "execute",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(4), "{run:?}");
    assert_eq!(run.caught_errors.len(), 3);
    assert!(run.failure_details.is_none());
    assert_eq!(run.heap_objects, 0);
    assert_eq!(run.rwx_mappings, 0);
    assert_eq!(run.published_code_versions, run.retired_code_versions);
    assert!(run.peak_root_frames >= 6);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn shared_lowering_emits_stack_maps_for_jit_and_aot() {
    let jit = run_jit_spike().unwrap();
    let aot = emit_aot_object().unwrap();
    assert_eq!(jit.result, 42);
    assert!(jit.executable_mappings > 0);
    assert!(jit.executable_bytes >= jit.compile.code_size);
    assert!(jit.compile.stack_map_count > 0);
    assert!(jit.compile.stack_map_entry_count > 0);
    assert!(aot.compile.stack_map_count > 0);
    assert!(aot.compile.stack_map_entry_count > 0);
    assert!(!aot.object.is_empty());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn baseline_jit_executes_source_calls_control_flow_allocation_and_runtime_polls() {
    let source = SourceFile::new(
        FileId(1),
        "baseline.rinzu",
        r#"
fn increment(value: Int) -> Int {
    return value + 1;
}

fn main() {
    let values: List<Int> = [1, 2];
    var counter = 0;
    while counter < 3 {
        counter = increment(counter);
    }
    if counter == 3 {
        let success = counter;
    } else {
        let alternate = 0;
    }
}
"#,
    );
    let artifact = compile_library(&source).expect("baseline fixture must compile");
    let run = run_baseline_jit(&artifact.ir, "main").expect("baseline JIT must run");
    assert_eq!(run.result, Ok(0));
    assert!(run.function_trace.starts_with(&["main".into()]));
    assert!(
        run.function_trace
            .iter()
            .filter(|name| *name == "increment")
            .count()
            >= 3
    );
    assert!(run.poll_count > 0);
    assert_eq!(run.allocation_count, 1);
    assert_eq!(run.heap_objects, 0);
    assert!(run.dispatch_resolutions >= 3);
    assert_eq!(run.callback_dispatches, 1);
    assert!(run.dispatch_versions.iter().all(|version| *version == 1));
    assert_eq!(run.published_code_versions, run.retired_code_versions);
    assert!(run.peak_live_code_frames > 0);
    assert!(run.executable_mappings > 0);
    assert_eq!(run.icache_flushes, run.executable_mappings);
    assert_eq!(run.rx_transitions, run.executable_mappings);
    assert_eq!(run.rwx_mappings, 0);
    assert!(run.compile.code_size > 0);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn baseline_jit_unwinds_arithmetic_failure_through_finally() {
    let source = SourceFile::new(
        FileId(2),
        "failure.rinzu",
        r#"
fn cleanup() {
}

fn main() {
    let zero = 0;
    try {
        let invalid = 1 / zero;
    } finally {
        cleanup();
    }
}
"#,
    );
    let artifact = compile_library(&source).expect("failure fixture must compile");
    let run = run_baseline_jit(&artifact.ir, "main").expect("baseline JIT must run");
    assert_eq!(run.result, Err(BaselineFailure::Arithmetic));
    assert!(run.function_trace.iter().any(|name| name == "cleanup"));
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn baseline_jit_collects_each_allocation_and_cancels_cpu_loop_through_finally() {
    let source = SourceFile::new(
        FileId(3),
        "stress.rinzu",
        r#"
fn cleanup() {
}

fn main() {
    var counter = 0;
    try {
        while true {
            let values: List<Int> = [counter];
            counter = counter + 1;
        }
    } finally {
        cleanup();
    }
}
"#,
    );
    let artifact = compile_library(&source).expect("stress fixture must compile");
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            collect_every_allocation: true,
            cancel_after_polls: Some(12),
            ..BaselineOptions::default()
        },
    )
    .expect("baseline JIT stress run must terminate");
    assert_eq!(run.result, Err(BaselineFailure::Cancelled));
    assert!(run.allocation_count > 0);
    assert!(run.collection_count >= run.allocation_count);
    assert_eq!(run.heap_objects, 0);
    assert!(run.function_trace.iter().any(|name| name == "cleanup"));
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
#[test]
fn unsupported_host_does_not_claim_v1_jit_support() {
    assert!(run_jit_spike().is_err());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn compiled_composites_preserve_payload_dispatch_equality_and_moving_roots() {
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "composite.rinzu",
        include_str!("fixtures/composite.rinzu"),
    ))
    .unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "verify",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(42), "{run:?}");
    assert_eq!(run.heap_objects, 0);
    assert!(run.allocation_count > 500);
    assert!(run.collection_count >= run.allocation_count);
    assert!(run.peak_heap_objects < 60, "{run:?}");
    assert!(run.peak_root_frames >= 3);
    assert_eq!(run.published_code_versions, run.retired_code_versions);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn typed_scalar_values_execute_with_width_and_ieee_semantics() {
    let cases = [
        ("CUInt64", "18446744073709551615", -1),
        ("CInt8", "-128", -128),
        ("CUInt8", "255", 255),
        ("CInt16", "-32768", -32768),
        ("CUInt16", "65535", 65535),
        ("CInt32", "-2147483648", -2147483648),
        ("CUInt32", "4294967295", 4294967295),
        ("Bool", "18446744073709551615 > 9223372036854775808", 1),
        ("CFloat", "(16777216.0 + 1.0) - 16777216.0", 0),
        ("Bool", "(0.0 / 0.0) != (0.0 / 0.0)", 1),
        ("Bool", "-0.0 == 0.0", 1),
    ];
    for (ty, expression, expected) in cases {
        let expression = if ty == "Bool" && expression.starts_with("184") {
            "high > low"
        } else {
            expression
        };
        let source = SourceFile::new(
            FileId(1),
            "scalar.rinzu",
            format!(
                "fn value()->{ty} {{ let high: CUInt64 = 18446744073709551615; let low: CUInt64 = 9223372036854775808; return {expression}; }} fn main() {{}}"
            ),
        );
        let artifact = compile_library(&source).unwrap();
        assert_eq!(
            run_baseline_jit(&artifact.ir, "value").unwrap().result,
            Ok(expected),
            "{ty}: {expression}"
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn typed_integer_arithmetic_traps_at_each_width() {
    for (ty, value) in [
        ("CInt8", "127"),
        ("CUInt8", "255"),
        ("CInt16", "32767"),
        ("CUInt16", "65535"),
        ("CInt32", "2147483647"),
        ("CUInt32", "4294967295"),
        ("CInt64", "9223372036854775807"),
        ("CUInt64", "18446744073709551615"),
    ] {
        let source = SourceFile::new(
            FileId(1),
            "overflow.rinzu",
            format!(
                "fn value()->{ty} {{ let value: {ty} = {value}; return value + 1; }} fn main() {{}}"
            ),
        );
        let artifact = compile_library(&source).unwrap();
        assert_eq!(
            run_baseline_jit(&artifact.ir, "value").unwrap().result,
            Err(BaselineFailure::Arithmetic),
            "{ty}"
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn module_integer_constants_are_baked_into_executable_ir() {
    let source = SourceFile::new(
        FileId(1),
        "constants.rinzu",
        "let answer = next + 1; let next = 41; fn value()->Int { return answer; } fn main() {}",
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit(&artifact.ir, "value").unwrap();
    assert_eq!(run.result, Ok(42));
    assert_eq!(run.allocation_count, 0);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn source_native_calls_require_a_validated_physical_binding() {
    let file = rinzu_compiler::SourceFile::new(
        rinzu_compiler::FileId(0),
        "native.rz",
        "native { fn external() -> Int symbol \"external\"; } fn main() { let result = external(); }",
    );
    let artifact = compile_library(&file).unwrap();
    let error = rinzu_codegen::run_baseline_jit(&artifact.ir, "main").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("source native binding is unavailable")
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn numeric_conversion_errors_are_caught_and_propagated_across_managed_calls() {
    for (body, expected) in [
        ("return CInt64(CInt8(127));", Ok(127)),
        (
            "try { let bad = CInt8(200); } catch error { return 42; } return 0;",
            Ok(42),
        ),
        (
            "try { let bad = convert(); } catch error { return 43; } return 0;",
            Ok(43),
        ),
        (
            "return CInt64(CInt8(200));",
            Err(BaselineFailure::Conversion),
        ),
        (
            "try { let trap = 1 / 0; } catch error { return 44; } return 0;",
            Err(BaselineFailure::Arithmetic),
        ),
    ] {
        let source = SourceFile::new(
            FileId(1),
            "conversion.rinzu",
            format!(
                "fn convert()->CInt8 {{ return CInt8(200); }} fn value()->Int {{ {body} }} fn main() {{}}"
            ),
        );
        let artifact = compile_library(&source).unwrap();
        assert_eq!(
            run_baseline_jit(&artifact.ir, "value").unwrap().result,
            expected,
            "{body}"
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn nested_finally_routes_errors_to_outer_catch_and_traps_past_catch() {
    for (operation, expected, caught) in [
        ("let bad = CInt8(200);", Ok(0), true),
        ("let bad = 1 / 0;", Err(BaselineFailure::Arithmetic), false),
    ] {
        let source = SourceFile::new(
            FileId(1),
            "finally.rinzu",
            format!(
                r#"
fn inner() {{}} fn outer() {{}} fn caught() {{}}
fn main() {{
    try {{
        try {{ {operation} }} finally {{ inner(); }}
    }} catch error {{ caught(); }} finally {{ outer(); }}
}}
"#
            ),
        );
        let artifact = compile_library(&source).unwrap();
        let run = run_baseline_jit(&artifact.ir, "main").unwrap();
        assert_eq!(run.result, expected);
        let relevant: Vec<_> = run
            .function_trace
            .iter()
            .filter(|name| ["inner", "outer", "caught"].contains(&name.as_str()))
            .map(String::as_str)
            .collect();
        assert_eq!(
            relevant,
            if caught {
                vec!["inner", "caught", "outer"]
            } else {
                vec!["inner", "outer"]
            }
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn finally_with_nested_control_flow_finishes_before_resuming_error() {
    let source = SourceFile::new(
        FileId(1),
        "nested-cleanup.rinzu",
        r#"
fn first() {} fn second() {} fn third() {} fn caught() {}
fn main() {
    try {
        try { let bad = CInt8(200); }
        finally {
            try { first(); } finally { second(); }
            if true { third(); }
        }
    } catch error { caught(); }
}
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit(&artifact.ir, "main").unwrap();
    assert_eq!(run.result, Ok(0));
    let relevant: Vec<_> = run
        .function_trace
        .iter()
        .filter(|name| name.as_str() != "main")
        .map(String::as_str)
        .collect();
    assert_eq!(relevant, ["first", "second", "third", "caught"]);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn cleanup_failures_preserve_primary_and_do_not_become_catchable_traps() {
    use rinzu_runtime::failure::{ErrorKind as E, FailureCause as C};
    for (primary, cleanup, expected, primary_cause, cleanup_cause, caught) in [
        (
            "let bad = CInt8(200);",
            "let other = CInt8(201);",
            Ok(0),
            C::Error(E::Conversion),
            C::Error(E::Conversion),
            true,
        ),
        (
            "let bad = CInt8(200);",
            "let other = 1 / 0;",
            Err(BaselineFailure::Conversion),
            C::Error(E::Conversion),
            C::ArithmeticTrap,
            false,
        ),
        (
            "let bad = 1 / 0;",
            "let other = CInt8(201);",
            Err(BaselineFailure::Arithmetic),
            C::ArithmeticTrap,
            C::Error(E::Conversion),
            false,
        ),
    ] {
        let source = SourceFile::new(
            FileId(1),
            "cleanup-failure.rinzu",
            format!(
                r#"
fn skipped() {{}} fn caught() {{}} fn outer() {{}}
fn main() {{ try {{ try {{ {primary} }} finally {{ {cleanup} skipped(); }} }} catch error {{ caught(); }} finally {{ outer(); }} }}
"#
            ),
        );
        let artifact = compile_library(&source).unwrap();
        let run = run_baseline_jit(&artifact.ir, "main").unwrap();
        assert_eq!(run.result, expected);
        assert!(!run.function_trace.iter().any(|name| name == "skipped"));
        assert_eq!(
            run.function_trace.iter().any(|name| name == "caught"),
            caught
        );
        assert_eq!(
            run.function_trace
                .iter()
                .filter(|name| name.as_str() == "outer")
                .count(),
            1
        );
        let failure = if caught {
            &run.caught_errors[0]
        } else {
            run.failure_details.as_ref().unwrap()
        };
        assert_eq!(failure.primary.cause, primary_cause);
        assert_eq!(
            failure.cleanup.iter().map(|d| d.cause).collect::<Vec<_>>(),
            [cleanup_cause]
        );
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn local_catch_in_finally_preserves_the_original_outer_unwind() {
    let source = SourceFile::new(
        FileId(1),
        "cleanup-catch.rinzu",
        r#"
fn local() {} fn outer() {} fn done() {}
fn main() {
    try { try { let bad = CInt8(200); }
        finally { try { let other = CInt8(201); } catch localError { local(); } done(); }
    } catch outerError { outer(); }
}
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit(&artifact.ir, "main").unwrap();
    assert_eq!(run.result, Ok(0));
    assert_eq!(run.function_trace, ["main", "local", "done", "outer"]);
    assert_eq!(run.caught_errors.len(), 2);
    assert!(
        run.caught_errors
            .iter()
            .all(|failure| failure.cleanup.is_empty())
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn cancellation_keeps_cleanup_error_diagnostics_and_bypasses_catch() {
    use rinzu_runtime::failure::{ErrorKind as E, FailureCause as C};
    let source = SourceFile::new(
        FileId(1),
        "cancel-cleanup.rinzu",
        r#"
fn caught() {} fn outer() {}
fn main() { try { try { while true { let tick = 1; } }
    finally { let bad = CInt8(200); }
} catch error { caught(); } finally { outer(); } }
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            cancel_after_polls: Some(6),
            ..BaselineOptions::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Err(BaselineFailure::Cancelled));
    assert!(!run.function_trace.iter().any(|name| name == "caught"));
    assert_eq!(
        run.function_trace
            .iter()
            .filter(|name| name.as_str() == "outer")
            .count(),
        1
    );
    let failure = run.failure_details.unwrap();
    assert_eq!(failure.primary.cause, C::Cancelled);
    assert_eq!(
        failure.cleanup.iter().map(|d| d.cause).collect::<Vec<_>>(),
        [C::Error(E::Conversion)]
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn diagnostic_message_and_strings_survive_calls_scopes_and_moving_gc() {
    let source = SourceFile::new(
        FileId(1),
        "message.rinzu",
        r#"
import rinzu.core;
fn retain(text: String, depth: Int) -> String {
    let temporary = "callee temporary";
    if depth == 0 { return text; }
    return retain(text, depth - 1);
}
fn message() -> String {
    try { let bad = CInt8(200); } catch [error: Error, ...errorRest] { return error.message; }
    return "";
}
fn value() -> Int {
    let received = message();
    var iteration = 0;
    while iteration < 100 {
        let copy = retain("日本語🙂\u{0}tail", 3);
        if copy != "日本語🙂\u{0}tail" { panic("payload_failure"); }
        if utf8ByteLength(copy) != 18 { panic("length_failure"); }
        iteration = iteration + 1;
    }
    if received != "numeric conversion is out of range or loses required integer precision" {
        panic("message_failure");
    }
    if "é" == "e\u{301}" { panic("normalization_failure"); }
    if "" != "" { panic("empty_failure"); }
    if "a\u{0}b" == "a" { panic("nul_failure"); }
    return Int(utf8ByteLength(received));
}
fn main() {}
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "value",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(70));
    assert_eq!(run.caught_errors.len(), 1);
    assert_eq!(run.heap_objects, 0);
    assert!(run.allocation_count > 500);
    assert!(run.peak_heap_objects < 20, "{run:?}");
    assert!(run.collection_count > 500);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn recursive_managed_calls_publish_roots_and_balance_each_frame() {
    let source = SourceFile::new(
        FileId(1),
        "root-frames.rinzu",
        r#"
fn recurse(depth: Int) -> Int { if depth == 0 { return 0; } return recurse(depth - 1) + 1; }
fn value() -> Int { return recurse(8); }
fn main() {}
"#,
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit(&artifact.ir, "value").unwrap();
    assert_eq!(run.result, Ok(8));
    assert_eq!(run.peak_root_frames, 10);
    assert!(run.root_publications >= 9);
    assert_eq!(
        run.poll_count, run.root_publications,
        "one poll per publication: {run:?}"
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn typed_length_and_managed_references_keep_owner_roots_and_dispatch_versions() {
    let source = SourceFile::new(
        FileId(1),
        "function-reference.rinzu",
        include_str!("fixtures/function_reference.rinzu"),
    );
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "verify",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(42));
    assert_eq!(run.caught_errors.len(), 2);
    assert_eq!(run.heap_objects, 0);
    assert!(run.peak_root_frames >= 3 && run.collection_count > 50);
    assert_eq!(run.published_code_versions, run.retired_code_versions);
    let aot = rinzu_codegen::emit_baseline_aot_object(&artifact.ir, "main").unwrap();
    assert!(
        aot.reference_targets
            .contains(&"rinzu.core::byteLength:4".to_owned())
    );
    assert!(
        aot.reference_targets
            .contains(&"rinzu.core::byteLength:0".to_owned())
    );
    assert!(
        aot.reference_targets
            .contains(&"rinzu.core::utf8ByteLength".to_owned())
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn primary_and_cleanup_diagnostics_keep_the_producing_source_across_calls() {
    use rinzu_runtime::failure::{Diagnostic, ErrorKind, FailureCause, SourcePosition};
    let text = r#"
fn initial() { let bad = CInt8(200); }
fn firstCleanup() { let other = CInt8(201); }
fn lastCleanup() { let overflow = 1 / 0; }
fn wrapper() {
    try { try { initial(); } finally { firstCleanup(); } }
    finally { lastCleanup(); }
}
fn main() { try { wrapper(); } catch error { let wrong = 42; } }
"#;
    let source = SourceFile::new(FileId(23), "diagnostic-positions.rinzu", text);
    let artifact = compile_library(&source).unwrap();
    let run = run_baseline_jit(&artifact.ir, "main").unwrap();
    let diagnostic = |expression: &str, cause| {
        let start = text.find(expression).unwrap();
        Diagnostic {
            cause,
            source: Some(SourcePosition {
                file: 1,
                start_byte: start as u32,
                end_byte: (start + expression.len()) as u32,
            }),
        }
    };
    let failure = run.failure_details.unwrap();
    assert_eq!(run.source_files[&1], "diagnostic-positions.rinzu");
    assert_eq!(
        failure.primary,
        diagnostic("CInt8(200)", FailureCause::Error(ErrorKind::Conversion))
    );
    assert_eq!(
        failure.cleanup,
        [
            diagnostic("CInt8(201)", FailureCause::Error(ErrorKind::Conversion)),
            diagnostic("1 / 0", FailureCause::ArithmeticTrap),
        ]
    );
    assert!(run.caught_errors.is_empty());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn compiled_fixed_bytes_padding_lengths_and_collection_keys_keep_exact_values() {
    let artifact = compile_library(&SourceFile::new(
        FileId(1),
        "bytes_collection.rinzu",
        include_str!("fixtures/bytes_collection.rinzu"),
    ))
    .unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "verify",
        BaselineOptions {
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(42), "{run:?}");
    assert_eq!(run.heap_objects, 0);
    assert!(run.allocation_count > 50);
    assert!(run.collection_count >= run.allocation_count);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn aggregate_stdout_keeps_caller_roots_through_moving_collections_and_callee_return() {
    let artifact = compile_library(&SourceFile::new(FileId(1),"display-gc.rinzu",r#"
import rinzu.stdio;
class Packet { const text: String, const bytes: Bytes(4) }
fn show(values: Map<Int, List<Packet>>) {
    var iteration = 0;
    while iteration < 20 { let garbage = Packet(text: "temporary", bytes: b"abcd"); iteration = iteration + 1; }
    rinzu.stdio.writeLine(values);
}
fn main() {
    let packet = Packet(text: "日本語\u{0}🙂", bytes: b"A\xff");
    let values: Map<Int, List<Packet>> = {2: [packet], 1: [packet]};
    show(values); rinzu.stdio.writeLine(packet);
}
"#)).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0));
    let packet = r#"Packet(text: "日本語\u{00}🙂", bytes: b"A\xff\x00\x00")"#;
    assert_eq!(
        run.stdout,
        format!("{{1: [{packet}], 2: [{packet}]}}\n{packet}\n").as_bytes()
    );
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count >= run.allocation_count);
}
