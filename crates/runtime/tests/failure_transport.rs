use rinzu_runtime::failure::{Diagnostic, ErrorKind, Failure, FailureCause as C, SourcePosition};
use rinzu_runtime::failure_transport::{failure_descriptor, failure_value, materialize_failure};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::transport::{
    TransferValue, TransportBudget, TransportDirection, encode, preflight,
};

fn failures() -> Vec<Failure> {
    let causes = [
        C::ArithmeticTrap,
        C::Cancelled,
        C::Overloaded,
        C::Shutdown,
        C::Error(ErrorKind::Conversion),
        C::Unsupported,
        C::RuntimePanic,
        C::Panic,
        C::Error(ErrorKind::NullNativeValue),
        C::Error(ErrorKind::InvalidUtf8),
        C::Error(ErrorKind::NativeRecordBusy),
        C::Error(ErrorKind::UnobservedChildFailure),
        C::Error(ErrorKind::Io),
        C::Error(ErrorKind::SupplantBusy),
        C::Error(ErrorKind::UnboundValue),
    ];
    causes
        .into_iter()
        .enumerate()
        .map(|(index, cause)| Failure {
            managed: Default::default(),
            traces: Default::default(),
            primary: Diagnostic {
                cause,
                source: (index % 2 == 0).then_some(SourcePosition {
                    file: u64::MAX,
                    start_byte: u32::MAX - 1,
                    end_byte: u32::MAX,
                }),
            },
            cleanup: causes
                .into_iter()
                .enumerate()
                .map(|(index, cause)| Diagnostic {
                    cause,
                    source: (index % 2 == 1).then_some(SourcePosition {
                        file: index as u64,
                        start_byte: index as u32,
                        end_byte: index as u32 + 1,
                    }),
                })
                .collect(),
        })
        .chain([Failure::new(C::Cancelled)])
        .chain(traced_failures())
        .collect()
}

fn traced_failures() -> Vec<Failure> {
    use rinzu_runtime::error_trace::{ErrorTrace, TraceFrame};
    use std::sync::Arc;
    let mut failure = Failure::new(C::Error(ErrorKind::Conversion));
    failure.append_cleanup(Failure::new(C::Error(ErrorKind::Io)));
    failure.traces.insert(
        0,
        Arc::new(ErrorTrace {
            frames: vec![
                TraceFrame {
                    function: Arc::from("変換\0leaf"),
                    source: Some(SourcePosition {
                        file: 17,
                        start_byte: 23,
                        end_byte: 31,
                    }),
                },
                TraceFrame {
                    function: Arc::from("caller"),
                    source: None,
                },
            ]
            .into(),
        }),
    );
    failure
        .traces
        .insert(1, Arc::new(ErrorTrace { frames: [].into() }));
    let mut cleanup_only = failure.clone();
    cleanup_only.traces.remove(&0);
    vec![failure, cleanup_only]
}

#[test]
fn immutable_failure_uses_exact_credit_until_receiver_decodes() {
    let descriptor = failure_descriptor();
    let budget = TransportBudget::new(1024, 1024);
    for failure in failures() {
        let value = failure_value(&failure);
        let size = preflight(&descriptor, &value).unwrap();
        let trace_bytes: usize = failure
            .traces
            .values()
            .map(|trace| {
                16 + trace
                    .frames
                    .iter()
                    .map(|frame| 25 + frame.function.len())
                    .sum::<usize>()
            })
            .sum();
        assert_eq!(size, 34 + 18 * failure.cleanup.len() + trace_bytes);
        let buffer = encode(
            &descriptor,
            &value,
            budget.reserve(TransportDirection::Result, size).unwrap(),
        )
        .unwrap();
        assert_eq!(budget.snapshot().used_bytes, size);
        let receiver_descriptor = descriptor.clone();
        let decoded = std::thread::spawn(move || {
            materialize_failure(buffer.materialize(&receiver_descriptor).unwrap())
        })
        .join()
        .unwrap()
        .unwrap();
        assert_eq!(decoded, failure);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }
    let mut invalid = failure_value(&Failure::new(C::Cancelled));
    let TransferValue::Record(fields) = &mut invalid else {
        unreachable!()
    };
    let TransferValue::Record(primary) = &mut fields[0] else {
        unreachable!()
    };
    primary[0] = TransferValue::Scalar(ScalarValue::from_bits(ScalarKind::U8, 99).unwrap());
    let size = preflight(&descriptor, &invalid).unwrap();
    let buffer = encode(
        &descriptor,
        &invalid,
        budget.reserve(TransportDirection::Result, size).unwrap(),
    )
    .unwrap();
    assert!(materialize_failure(buffer.materialize(&descriptor).unwrap()).is_err());
    assert_eq!(budget.snapshot().used_bytes, 0);
    let buffer = encode(
        &descriptor,
        &failure_value(&Failure::new(C::Cancelled)),
        budget.reserve(TransportDirection::Result, 34).unwrap(),
    )
    .unwrap();
    let wrong_descriptor = rinzu_runtime::transport::TransferDescriptor {
        kind: rinzu_runtime::transport::TransferKind::Unit,
        source: None,
    };
    assert!(buffer.materialize(&wrong_descriptor).is_err());
    assert_eq!(budget.snapshot().used_bytes, 0);
}

#[test]
fn failure_packet_rejects_noncanonical_source_and_wrong_value_shapes() {
    let failure = Failure {
        managed: Default::default(),
        traces: Default::default(),
        primary: Diagnostic {
            cause: C::Cancelled,
            source: Some(SourcePosition {
                file: 1,
                start_byte: 10,
                end_byte: 9,
            }),
        },
        cleanup: vec![],
    };
    assert!(materialize_failure(failure_value(&failure)).is_err());
    assert!(materialize_failure(TransferValue::Unit).is_err());
    assert!(materialize_failure(TransferValue::Record(vec![])).is_err());
    let mut value = failure_value(&Failure::new(C::Cancelled));
    let TransferValue::Record(fields) = &mut value else {
        unreachable!()
    };
    let TransferValue::Record(primary) = &mut fields[0] else {
        unreachable!()
    };
    primary[2] = TransferValue::Scalar(ScalarValue::from_bits(ScalarKind::U64, 1).unwrap());
    assert!(materialize_failure(value).is_err());
}

#[test]
fn aot_failure_packets_match_rust_and_reject_partial_publication() {
    use std::fmt::Write;
    let descriptor = failure_descriptor();
    let budget = TransportBudget::new(1024, 1024);
    let mut program = String::from(
        "#include <assert.h>\n#include <string.h>\n#include \"rinzu/failure_transport.h\"\nint main(void) {\n",
    );
    for failure in failures() {
        let value = failure_value(&failure);
        let size = preflight(&descriptor, &value).unwrap();
        let buffer = encode(
            &descriptor,
            &value,
            budget.reserve(TransportDirection::Result, size).unwrap(),
        )
        .unwrap();
        let bytes = buffer
            .bytes()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        writeln!(program, "{{ const unsigned char expected[] = {{{bytes}}}; unsigned char actual[sizeof(expected)]; rinzu_failure_t decoded = {{0}}; assert(!rinzu_failure_transfer_decode(expected, sizeof(expected), &decoded)); assert(rinzu_failure_transfer_size(&decoded) == sizeof(expected)); assert(!rinzu_failure_transfer_encode(&decoded, actual, sizeof(actual))); assert(!memcmp(actual, expected, sizeof(actual)));").unwrap();
        writeln!(
            program,
            "  const size_t cleanup_count = {};",
            failure.cleanup.len()
        )
        .unwrap();
        program.push_str(r#"
  rinzu_failure_t guard = {0};
  assert(rinzu_failure_transfer_decode(expected, sizeof(expected)-1, &guard) == RINZU_TRANSFER_LENGTH);
  assert(!guard.primary && !guard.head && !guard.tail);
  unsigned char invalid[sizeof(expected) + 18]; memcpy(invalid, expected, sizeof(expected));
  assert(rinzu_failure_transfer_decode(invalid, sizeof(invalid), &guard) == RINZU_TRANSFER_LENGTH);
  invalid[18] += 1;
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) != RINZU_TRANSFER_OK);
  memcpy(invalid, expected, sizeof(expected)); invalid[0] = 0;
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  invalid[0] = 17;
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  memcpy(invalid, expected, sizeof(expected)); invalid[1] = 2;
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  memcpy(invalid, expected, sizeof(expected)); invalid[1] = 0; invalid[2] = 1;
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  memcpy(invalid, expected, sizeof(expected)); invalid[1] = 1;
  memset(invalid + 10, 255, 4); memset(invalid + 14, 0, 4);
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  assert(!guard.primary && !guard.head && !guard.tail);
  guard.primary = 7;
  assert(rinzu_failure_transfer_decode(expected, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  assert(guard.primary == 7 && !guard.head);
  if (cleanup_count) {
    memcpy(invalid, expected, sizeof(expected)); invalid[26+(cleanup_count-1)*18] = 99;
    guard = (rinzu_failure_t){0};
    assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
    assert(!guard.primary && !guard.head && !guard.tail);
    decoded.tail->cause = 99; memset(actual, 77, sizeof(actual));
    assert(rinzu_failure_transfer_encode(&decoded, actual, sizeof(actual)) == RINZU_TRANSFER_ENCODING);
    for (size_t i = 0; i < sizeof(actual); ++i) assert(actual[i] == 77);
  }
"#);
        if failure.traces.contains_key(&0) {
            let offset = 34 + 18 * failure.cleanup.len();
            writeln!(program, r#"
  guard = (rinzu_failure_t){{0}};
  memcpy(invalid, expected, sizeof(expected));
  rinzu_failure_transfer_u64(invalid+{offset}, cleanup_count+1);
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  assert(!guard.primary && !guard.primary_trace && !guard.head);
  memcpy(invalid, expected, sizeof(expected));
  rinzu_failure_transfer_u64(invalid+{offset}+8, UINT64_MAX);
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_LENGTH);
  memcpy(invalid, expected, sizeof(expected));
  rinzu_failure_transfer_u64(invalid+{offset}+16, UINT64_MAX);
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_LENGTH);
  memcpy(invalid, expected, sizeof(expected)); invalid[{offset}+24]=255;
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  memcpy(invalid, expected, sizeof(expected));
  rinzu_failure_transfer_u64(invalid+sizeof(expected)-16, 0);
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  memcpy(invalid, expected, sizeof(expected));
  rinzu_failure_transfer_u64(invalid+{offset}, 1);
  rinzu_failure_transfer_u64(invalid+sizeof(expected)-16, 0);
  assert(rinzu_failure_transfer_decode(invalid, sizeof(expected), &guard) == RINZU_TRANSFER_ENCODING);
  assert(!guard.primary && !guard.primary_trace && !guard.head);
  rinzu_failure_t copied=rinzu_failure_copy(&decoded);
  assert(copied.primary_trace==decoded.primary_trace);
  rinzu_failure_release(copied);
"#).unwrap();
        }
        program.push_str("  rinzu_failure_release(decoded);\n}\n");
    }
    program.push_str("return 0; }\n");
    let root =
        std::env::temp_dir().join(format!("rinzu-c-failure-transport-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"), program).unwrap();
    for sanitizer in [false, true] {
        let mut command = std::process::Command::new("clang");
        command.args(["-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror"]);
        if sanitizer {
            command.arg("-fsanitize=address,undefined");
        }
        let output = command
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(root.join("test.c"))
            .arg("-o")
            .arg(root.join("test"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = std::process::Command::new(root.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(budget.snapshot().used_bytes, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn trace_packets_reject_duplicate_out_of_order_and_out_of_range_diagnostic_indices() {
    let original = failure_value(&traced_failures()[0]);
    for (first, second) in [(0, 0), (1, 0), (0, 2)] {
        let mut invalid = original.clone();
        let TransferValue::Record(fields) = &mut invalid else {
            unreachable!()
        };
        let TransferValue::PersistentList(entries) = &mut fields[2] else {
            unreachable!()
        };
        for (entry, index) in entries.iter_mut().zip([first, second]) {
            let TransferValue::Record(fields) = entry else {
                unreachable!()
            };
            fields[0] =
                TransferValue::Scalar(ScalarValue::from_bits(ScalarKind::U64, index).unwrap());
        }
        assert!(materialize_failure(invalid).is_err());
    }
    let mut invalid = original;
    let TransferValue::Record(fields) = &mut invalid else {
        unreachable!()
    };
    let TransferValue::PersistentList(entries) = &mut fields[2] else {
        unreachable!()
    };
    let TransferValue::Record(entry) = &mut entries[0] else {
        unreachable!()
    };
    let TransferValue::PersistentList(frames) = &mut entry[1] else {
        unreachable!()
    };
    let TransferValue::Record(frame) = &mut frames[0] else {
        unreachable!()
    };
    let TransferValue::Record(source) = &mut frame[1] else {
        unreachable!()
    };
    source[0] = TransferValue::Scalar(ScalarValue::from_bits(ScalarKind::Bool, 0).unwrap());
    assert!(materialize_failure(invalid).is_err());
}
