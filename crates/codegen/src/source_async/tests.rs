use super::*;
use crate::test_library::{
    compile_library as compile_source, compile_library_with_options as compile_source_with_options,
};
use rinzu_compiler::{CompilerOptions, FileId, NativeCallClass, NativeFunction, SourceFile, Type};

#[test]
fn scalar_transport_preserves_all_kinds_and_exact_widths_in_source_workers() {
    for (name, literal, kind, bits) in [
        ("CInt8", "-128", ScalarKind::I8, (-128_i64) as u64),
        ("CInt16", "-32768", ScalarKind::I16, (-32768_i64) as u64),
        (
            "CInt32",
            "-2147483648",
            ScalarKind::I32,
            (-2147483648_i64) as u64,
        ),
        (
            "CInt64",
            "-9223372036854775808",
            ScalarKind::I64,
            i64::MIN as u64,
        ),
        ("CUInt8", "255", ScalarKind::U8, 255),
        ("CUInt16", "65535", ScalarKind::U16, 65535),
        ("CUInt32", "4294967295", ScalarKind::U32, 4294967295),
        ("CSize", "18446744073709551615", ScalarKind::U64, u64::MAX),
        (
            "CFloat",
            "-0.0",
            ScalarKind::F32,
            u64::from((-0.0_f32).to_bits()),
        ),
        ("CDouble", "-0.0", ScalarKind::F64, (-0.0_f64).to_bits()),
        ("CBool", "true", ScalarKind::Bool, 1),
    ] {
        let source = SourceFile::new(
            FileId(1),
            "typed-worker.rinzu",
            format!(
                "fn echo(value: {name}) -> {name} {{ return value; }} async fn main() {{ let value: {name} = {literal}; let copy = await spawn echo(value); if copy != value {{ panic(\"failure\"); }} }}"
            ),
        );
        let artifact = compile_source(&source).unwrap();
        let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        let value = run.last_awaited_scalar.unwrap();
        assert_eq!(value.kind(), kind);
        assert_eq!(value.bits(), bits);
        assert_eq!(
            run.transport.peak_transport_bytes,
            kind.width() as usize / 8
        );
        assert!(run.resources_returned, "{run:?}");
    }
}

#[test]
fn mixed_scalar_worker_arguments_use_ieee_arithmetic_and_separate_input_credit() {
    let source = SourceFile::new(
        FileId(1),
        "mixed-worker.rinzu",
        r#"
fn calculate(narrow: CInt8, wide: CUInt64, value: CFloat, enabled: CBool) -> CFloat {
    if narrow != -128 { panic("narrow_failure"); }
    if wide <= 9223372036854775808 { panic("unsigned_failure"); }
    if !enabled { panic("bool_failure"); }
    return (value + 1.0) - value;
}
async fn main() {
    let result = await spawn calculate(-128, 18446744073709551615, 16777216.0, true);
    if result != 0.0 { panic("rounding_failure"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert_eq!(run.last_awaited_scalar.unwrap().kind(), ScalarKind::F32);
    assert_eq!(run.last_awaited_scalar.unwrap().bits(), 0);
    assert_eq!(run.transport.peak_transport_bytes, 14);
    assert!(run.resources_returned);
}

#[test]
fn ready_continuation_and_worker_preserve_nan_without_integer_reinterpretation() {
    let source = SourceFile::new(
        FileId(1),
        "nan-worker.rinzu",
        r#"
async fn ready() -> CFloat { return 0.0 / 0.0; }
fn echo(value: CFloat) -> CFloat {
    if value == value { panic("nan_failure"); }
    return value;
}
async fn main() {
    let value = await ready();
    let copy = await spawn echo(value);
    if copy == copy { panic("nan_failure"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    let value = run.last_awaited_scalar.unwrap();
    assert_eq!(value.kind(), ScalarKind::F32);
    assert!(value.float().unwrap().is_nan());
    let zero = ScalarValue::from_f32(0.0);
    assert_eq!(
        value.bits(),
        zero.arithmetic(Arithmetic::Divide, zero).unwrap().bits()
    );
    assert!(run.resources_returned);
}

#[test]
fn worker_strings_and_utf8_length_copy_in_both_directions() {
    let source = SourceFile::new(
        FileId(1),
        "string-worker.rinzu",
        r#"
import rinzu.core;
fn echo(value: String) -> String { return value; }
fn length(value: String) -> CSize { return utf8ByteLength(value); }
async fn main() {
    let copy = await spawn echo("日本語🙂\u{0}tail");
    if copy != "日本語🙂\u{0}tail" { panic("copy_failure"); }
    let count = await spawn length(copy);
    if count != 18 { panic("length_failure"); }
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert_eq!(run.last_awaited_scalar.unwrap().kind(), ScalarKind::U64);
    assert_eq!(run.last_awaited_result, Some(18));
    assert_eq!(run.transport.peak_transport_bytes, 26);
    assert!(run.resources_returned);
}

#[test]
fn worker_traps_and_cancellation_return_transport_and_admission_to_baseline() {
    for (body, cancel) in [
        (
            "let byte: CUInt8 = 255; let overflow = byte + 1; return count;",
            false,
        ),
        (
            "var value = count; while true { value = value + 1; } return value;",
            true,
        ),
    ] {
        let source = SourceFile::new(
            FileId(1),
            "worker-failure.rinzu",
            format!(
                "fn calculate(count: Int) -> Int {{ {body} }} async fn main() {{ let value = await spawn calculate(0); }}"
            ),
        );
        let artifact = compile_source(&source).unwrap();
        let admission = SpawnAdmissionController::new();
        assert!(
            run_source_async_admitted(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    cancel_first_spawn: cancel,
                    ..SourceAsyncOptions::default()
                },
                admission.clone()
            )
            .is_err()
        );
        let snapshot = admission.snapshot(OriginDomainId(0));
        assert_eq!(snapshot.worker_domains, 0);
        assert_eq!(snapshot.in_flight_for_origin, 0);
        assert_eq!(snapshot.transport_bytes, 0);
        if cancel {
            assert!([8, 26].contains(&snapshot.peak_transport_bytes));
        } else {
            assert_eq!(snapshot.peak_transport_bytes, 34);
        }
    }
}

#[test]
fn explicit_ready_query_is_recorded_for_cancellation_comparison() {
    let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "ready-observation.rinzu",
            "async fn ready()->Int{return 42;} async fn main(){let pending=ready();let done=futureReady(pending);let value=await pending;if done||value!=42 {panic(\"wrong\");}}",
        ))
        .unwrap();
    let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(run.control_attempts.len(), 1);
    assert!(run.events.iter().any(|event| matches!(event,
            SourceAsyncEvent::Awaited {future, ..} if *future==run.control_attempts[0])));
}

#[test]
fn first_await_starts_a_runtime_turn_and_ready_aliases_do_not_suspend_again() {
    let source = SourceFile::new(
        FileId(1),
        "ready-alias.rinzu",
        r#"
async fn ready()->Int {return 42;}
async fn invalid()->Int {let bad=CInt8(999);return 0;}
async fn main() {
    let value=ready();let alias=value;
    let first=await value;let second=await alias;
    if first!=42||second!=42 {panic("wrong");}
    let failure=invalid();var caught=0;
    try {let ignored=await failure;}catch one {caught=caught+1;}
    try {let ignoredAgain=await failure;}catch two {caught=caught+1;}
    if caught!=2 {panic("wrongCount");}
}
"#,
    );
    let artifact = compile_source(&source).unwrap();
    let run = run_source_async_report(&artifact.ir, "main", Default::default()).unwrap();
    assert!(run.failure_details.is_none() && run.resources_returned);
    assert_eq!(run.caught_errors.len(), 2);
    assert_eq!(
        run.events
            .iter()
            .filter(|event| matches!(event, SourceAsyncEvent::Suspended { .. }))
            .count(),
        2,
    );
    let observed = run
        .events
        .iter()
        .filter_map(|event| match event {
            SourceAsyncEvent::Awaited { scope, future } => Some((*scope, *future)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(observed, [(0, 0), (0, 0), (0, 1), (0, 1)]);
}

#[test]
fn source_spawn_suspends_and_resumes_on_owner_thread() {
    let source = SourceFile::from_utf8(
        FileId(1),
        "main.rinzu",
        br#"
fn cpuScore(size: Int) -> Int {
    return size * 2;
}

async fn main() {
    let score = await spawn cpuScore(20);
}
"#
        .to_vec(),
    )
    .unwrap();
    let artifact = compile_source(&source).unwrap();
    let owner = std::thread::current().id();
    let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
    assert_eq!(run.result, 0);
    assert_eq!(run.last_awaited_result, Some(40));
    assert!(run.resources_returned);
    assert!(run.events.iter().any(|event| matches!(
        event,
        SourceAsyncEvent::WorkerCompleted { worker_thread, .. } if *worker_thread != owner
    )));
    assert!(run.events.iter().any(|event| matches!(
        event,
        SourceAsyncEvent::Resumed { owner_thread, .. } if *owner_thread == owner
    )));
}

#[test]
fn source_file_read_uses_reactor_and_resumes_on_owner_thread() {
    let source = SourceFile::from_utf8(
        FileId(1),
        "main.rinzu",
        br#"
async fn main() {
    let size = await sdk.io.readSize("data.txt");
}
"#
        .to_vec(),
    )
    .unwrap();
    let mut compiler = CompilerOptions::default();
    compiler.native_functions.insert(
        "sdk.io.readSize".into(),
        NativeFunction {
            parameters: vec![Type::String],
            result: Type::Future(Box::new(Type::Int)),
            call_class: NativeCallClass::NativeSafe,
            metadata_complete: true,
            cleanup_safe: false,
        },
    );
    let artifact = compile_source_with_options(&source, &compiler).unwrap();
    let root = std::env::temp_dir().join(format!(
        "rinzu-source-async-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("data.txt"), b"rinzu").unwrap();
    let owner = std::thread::current().id();
    let run = run_source_async(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            package_root: root.clone(),
            native_async: BTreeMap::from([(
                "sdk.io.readSize".into(),
                NativeAsyncOperation::FileReadSize { path_argument: 0 },
            )]),
            ..SourceAsyncOptions::default()
        },
    )
    .unwrap();
    assert_eq!(run.last_awaited_result, Some(5));
    assert!(run.resources_returned);
    assert!(
        run.events
            .iter()
            .any(|event| matches!(event, SourceAsyncEvent::FileCompleted { bytes: 5, .. }))
    );
    assert!(run.events.iter().any(|event| matches!(
        event,
        SourceAsyncEvent::Resumed { owner_thread, .. } if *owner_thread == owner
    )));
    let submitted = run.events.iter().position(|event| matches!(event, SourceAsyncEvent::Submitted { operation, .. } if operation == "file-read-size")).unwrap();
    let awaited = run
        .events
        .iter()
        .position(|event| matches!(event, SourceAsyncEvent::Suspended { .. }))
        .unwrap();
    assert!(submitted < awaited);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_spawn_cancellation_is_not_detached() {
    let source = SourceFile::from_utf8(
        FileId(1),
        "main.rinzu",
        br#"
fn keepWorking(size: Int) -> Int {
    var value = 0;
    while value < size {
        value = value + 1;
    }
    return value;
}

async fn main() {
    let score = await spawn keepWorking(1000000);
}
"#
        .to_vec(),
    )
    .unwrap();
    let artifact = compile_source(&source).unwrap();
    let error = run_source_async(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            cancel_first_spawn: true,
            ..SourceAsyncOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
}
