use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as V};
use rinzu_runtime::transport::{
    TransferDescriptor, TransferKind, TransferValue, TransportBudget, TransportDirection, encode,
    preflight,
};

#[test]
fn all_scalar_widths_round_trip_through_typed_domain_transport() {
    let values = [
        V::from_integer(K::I8, -128).unwrap(),
        V::from_integer(K::I16, -32768).unwrap(),
        V::from_integer(K::I32, i32::MIN as i128).unwrap(),
        V::from_integer(K::I64, i64::MIN as i128).unwrap(),
        V::from_integer(K::U8, 255).unwrap(),
        V::from_integer(K::U16, 65535).unwrap(),
        V::from_integer(K::U32, u32::MAX as i128).unwrap(),
        V::from_integer(K::U64, u64::MAX as i128).unwrap(),
        V::from_f32(-0.0),
        V::from_f64(f64::INFINITY),
        V::from_bool(true),
        V::from_bits(K::F32, 0x7fc0_1234).unwrap(),
        V::from_bits(K::F64, 0x7ff8_0000_0000_1234).unwrap(),
    ];
    let budget = TransportBudget::new(1024, 1024);
    for value in values {
        let descriptor = TransferDescriptor {
            kind: TransferKind::Scalar(value.kind()),
            source: None,
        };
        let encoded_value = TransferValue::Scalar(value);
        let exact = preflight(&descriptor, &encoded_value).unwrap();
        assert_eq!(exact, value.kind().width() as usize / 8);
        for direction in [TransportDirection::Input, TransportDirection::Result] {
            let reservation = budget.reserve(direction, exact).unwrap();
            let buffer = encode(&descriptor, &encoded_value, reservation).unwrap();
            assert_eq!(buffer.bytes(), &value.bits().to_le_bytes()[..exact]);
            let worker_descriptor = descriptor.clone();
            let transferred = std::thread::spawn(move || buffer.materialize(&worker_descriptor))
                .join()
                .unwrap()
                .unwrap();
            let TransferValue::Scalar(decoded) = transferred else {
                panic!("scalar transport changed the value kind");
            };
            assert_eq!(decoded.kind(), value.kind());
            assert_eq!(decoded.bits(), value.bits());
            assert_eq!(budget.snapshot().used_bytes, 0);
        }
    }
    let descriptor = TransferDescriptor {
        kind: TransferKind::Scalar(K::U16),
        source: None,
    };
    assert!(
        preflight(
            &descriptor,
            &TransferValue::Scalar(V::from_integer(K::U8, 1).unwrap())
        )
        .is_err()
    );
}

#[test]
fn typed_integer_keys_preserve_full_values_and_normalized_type_identity() {
    let max = V::from_integer(K::U64, u64::MAX as i128).unwrap();
    let near = V::from_integer(K::U64, u64::MAX as i128 - 1).unwrap();
    let mut keys = std::collections::HashMap::new();
    keys.insert(max.key().unwrap(), "max");
    keys.insert(near.key().unwrap(), "near");
    assert_eq!(keys[&max.key().unwrap()], "max");
    assert_eq!(keys[&near.key().unwrap()], "near");
    assert_eq!(max.key(), V::from_bits(K::U64, u64::MAX).unwrap().key());
    assert_ne!(
        V::from_integer(K::I32, 1).unwrap().key(),
        V::from_integer(K::I64, 1).unwrap().key()
    );
    assert!(V::from_f32(1.0).key().is_none());
    assert!(V::from_bool(true).key().is_none());
}

#[test]
fn aot_scalar_transport_matches_rust_bytes_and_rejects_partial_parameters() {
    use std::fmt::Write;
    let mut values = Vec::new();
    for kind in [K::I8, K::I16, K::I32, K::I64, K::U8, K::U16, K::U32, K::U64] {
        let (minimum, maximum) = kind.bounds().unwrap();
        for integer in [minimum, 0, maximum] {
            values.push(V::from_integer(kind, integer).unwrap());
        }
    }
    values.extend([
        V::from_f32(-0.0),
        V::from_f32(f32::INFINITY),
        V::from_bits(K::F32, 0x7fc0_1234).unwrap(),
        V::from_bits(K::F32, 0x7f80_1234).unwrap(),
        V::from_f64(-0.0),
        V::from_f64(f64::INFINITY),
        V::from_bits(K::F64, 0x7ff8_0000_0000_1234).unwrap(),
        V::from_bool(false),
        V::from_bool(true),
    ]);
    let budget = TransportBudget::new(1024, 1024);
    let mut program = String::from(
        "#include <assert.h>\n#include <string.h>\n#include \"rinzu/scalar_transport.h\"\nint main(void) {\n",
    );
    let mut kinds = Vec::new();
    let mut slots = Vec::new();
    let mut record_bytes = Vec::new();
    for value in &values {
        let descriptor = TransferDescriptor {
            kind: TransferKind::Scalar(value.kind()),
            source: None,
        };
        let transfer = TransferValue::Scalar(*value);
        let size = preflight(&descriptor, &transfer).unwrap();
        let buffer = encode(
            &descriptor,
            &transfer,
            budget.reserve(TransportDirection::Input, size).unwrap(),
        )
        .unwrap();
        let bytes = buffer
            .bytes()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            program,
            "{{ unsigned char actual[{size}] = {{0}}; const unsigned char expected[] = {{{bytes}}};"
        )
        .unwrap();
        writeln!(program, "assert(rinzu_transfer_encode_scalar({}, UINT64_C({}), actual, sizeof(actual)) == RINZU_TRANSFER_OK);", value.kind() as u32, value.bits()).unwrap();
        writeln!(program, "assert(memcmp(actual, expected, sizeof(actual)) == 0); rinzu_scalar_result_t decoded = rinzu_transfer_decode_scalar({}, actual, sizeof(actual)); assert(!decoded.status && decoded.bits == UINT64_C({})); }}", value.kind() as u32, value.bits()).unwrap();
        kinds.push((value.kind() as u32).to_string());
        slots.push(format!("UINT64_C({})", value.bits()));
        record_bytes.extend_from_slice(buffer.bytes());
    }
    let bytes = record_bytes
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",");
    writeln!(program, "{{ const unsigned kinds[] = {{{}}}; const uint64_t expected_slots[] = {{{}}}; const unsigned char expected_bytes[] = {{{bytes}}}; unsigned char actual[sizeof(expected_bytes)]; uint64_t decoded[sizeof(kinds)/sizeof(kinds[0])]; size_t size = 0;", kinds.join(","), slots.join(",")).unwrap();
    program.push_str("assert(!rinzu_transfer_record_size(kinds, sizeof(kinds)/sizeof(kinds[0]), &size) && size == sizeof(expected_bytes));\nassert(!rinzu_transfer_encode_record(kinds, expected_slots, sizeof(kinds)/sizeof(kinds[0]), actual, size));\nassert(memcmp(actual, expected_bytes, size) == 0);\nassert(!rinzu_transfer_decode_record(kinds, sizeof(kinds)/sizeof(kinds[0]), actual, size, decoded));\nassert(memcmp(decoded, expected_slots, sizeof(decoded)) == 0); }\n");
    program.push_str(r#"
  assert(rinzu_transfer_decode_scalar(RINZU_TRANSFER_UNIT, NULL, 0).status == RINZU_TRANSFER_OK);
  assert(rinzu_transfer_encode_scalar(RINZU_TRANSFER_UNIT, 0, NULL, 0) == RINZU_TRANSFER_OK);
  assert(rinzu_transfer_encode_scalar(RINZU_TRANSFER_UNIT, 1, NULL, 0) == RINZU_TRANSFER_ENCODING);
  assert(rinzu_transfer_encode_record(NULL, NULL, 0, NULL, 0) == RINZU_TRANSFER_OK);
  assert(rinzu_transfer_decode_record(NULL, 0, NULL, 0, NULL) == RINZU_TRANSFER_OK);
  unsigned kinds[] = {RINZU_SCALAR_U8, RINZU_SCALAR_BOOL};
  uint64_t slots[] = {255, 2}, decoded[] = {123, 456};
  unsigned char bytes[8] = {77, 88};
  assert(rinzu_transfer_encode_record(kinds, slots, 2, bytes, 2) == RINZU_TRANSFER_ENCODING);
  assert(bytes[0] == 77 && bytes[1] == 88);
  bytes[0] = 255; bytes[1] = 2;
  assert(rinzu_transfer_decode_record(kinds, 2, bytes, 2, decoded) == RINZU_TRANSFER_ENCODING);
  assert(decoded[0] == 123 && decoded[1] == 456);
  assert(rinzu_transfer_decode_record(kinds, 2, bytes, 1, decoded) == RINZU_TRANSFER_LENGTH);
  assert(rinzu_transfer_decode_record(kinds, 2, bytes, 3, decoded) == RINZU_TRANSFER_LENGTH);
  assert(rinzu_transfer_encode_scalar(RINZU_SCALAR_I8, 128, bytes, 1) == RINZU_TRANSFER_ENCODING);
  assert(rinzu_transfer_encode_scalar(RINZU_SCALAR_F32, UINT64_MAX, bytes, 4) == RINZU_TRANSFER_ENCODING);
  assert(rinzu_transfer_decode_scalar(99, bytes, 1).status == RINZU_TRANSFER_KIND);
  assert(rinzu_transfer_decode_scalar(RINZU_SCALAR_U8, NULL, 1).status == RINZU_TRANSFER_LENGTH);
  return 0;
}
"#);
    let root = std::env::temp_dir().join(format!("rinzu-c-transport-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"), program).unwrap();
    let output = std::process::Command::new("clang")
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-I"])
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
