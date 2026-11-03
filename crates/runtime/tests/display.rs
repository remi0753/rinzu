#![cfg(unix)]
use rinzu_runtime::data::display::{DisplaySchema, DisplayType as D};
use rinzu_runtime::data::{ManagedData, ManagedSlot as S};
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType as V};
use std::sync::Arc;

fn scalar(kind: K, bits: u64) -> S {
    S::Scalar(ScalarValue::from_bits(kind, bits).unwrap())
}

#[test]
fn formatted_string_joins_display_values_and_survives_moving_gc() {
    let mut heap = DomainHeap::new(806);
    let mut data = ManagedData::new(806);
    data.configure_values(
        &heap,
        Arc::new(ValueSchema::new(vec![V::String, V::Scalar(K::U64), V::List(0)]).unwrap()),
    )
    .unwrap();
    let schema = DisplaySchema(vec![D::String, D::Scalar(K::U64), D::List(0)]);
    let text = data.create_string(&mut heap, "日本語\0{}").unwrap();
    let list = data
        .create_composite(&mut heap, 2, None, &[S::Reference(text)])
        .unwrap();
    let before = data.root_count();
    let result = data
        .format_string(
            &mut heap,
            &schema,
            &[
                (0, S::Reference(text)),
                (1, scalar(K::U64, u64::MAX)),
                (2, S::Reference(list)),
            ],
        )
        .unwrap();
    assert_eq!(data.root_count(), before + 1);
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(
        data.string(&heap, result).unwrap(),
        "日本語\0{}18446744073709551615[\"日本語\\u{00}{}\"]"
    );
    let empty = data.format_string(&mut heap, &schema, &[]).unwrap();
    assert_eq!(data.string(&heap, empty).unwrap(), "");
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(data.root_count(), 0);
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn shared_display_engine_passes_address_and_undefined_behavior_sanitizers() {
    let root =
        std::env::temp_dir().join(format!("rinzu-display-sanitizers-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = root.join("display");
    let compiled = std::process::Command::new("clang")
        .args([
            "-std=c11",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fsanitize=address,undefined",
            "-fno-omit-frame-pointer",
            "-pthread",
            "-I",
        ])
        .arg(manifest.join("../../include"))
        .arg(manifest.join("src/display.c"))
        .arg(manifest.join("tests/fixtures/display.c"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = std::process::Command::new(binary).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.stderr.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn scalar_text_preserves_width_precision_special_floats_and_unit() {
    let cases = [
        (K::I8, (-128_i64) as u64, "-128"),
        (K::I16, (-32768_i64) as u64, "-32768"),
        (K::I32, (i32::MIN as i64) as u64, "-2147483648"),
        (K::I64, i64::MIN as u64, "-9223372036854775808"),
        (K::I64, i64::MAX as u64, "9223372036854775807"),
        (K::U8, 255, "255"),
        (K::U16, 65535, "65535"),
        (K::U32, u32::MAX as u64, "4294967295"),
        (K::U64, u64::MAX, "18446744073709551615"),
        (K::Bool, 0, "false"),
        (K::Bool, 1, "true"),
        (K::F32, (0.1_f32).to_bits() as u64, "0.100000001"),
        (K::F64, (0.1_f64).to_bits(), "0.10000000000000001"),
        (K::F32, (-0.0_f32).to_bits() as u64, "-0.0"),
        (K::F64, (1.0_f64).to_bits(), "1.0"),
        (K::F64, f64::NAN.to_bits(), "NaN"),
        (K::F64, f64::INFINITY.to_bits(), "inf"),
        (K::F64, f64::NEG_INFINITY.to_bits(), "-inf"),
    ];
    let heap = DomainHeap::new(801);
    let mut data = ManagedData::new(801);
    let mut values: Vec<_> = cases.iter().map(|(kind, _, _)| V::Scalar(*kind)).collect();
    values.push(V::Unit);
    data.configure_values(&heap, Arc::new(ValueSchema::new(values).unwrap()))
        .unwrap();
    let mut types: Vec<_> = cases.iter().map(|(kind, _, _)| D::Scalar(*kind)).collect();
    types.push(D::Unit);
    let schema = DisplaySchema(types);
    for (index, (kind, bits, expected)) in cases.into_iter().enumerate() {
        assert_eq!(
            data.display(&heap, &schema, index, scalar(kind, bits))
                .unwrap(),
            expected.as_bytes()
        );
    }
    assert_eq!(
        data.display(&heap, &schema, cases.len(), S::Unit).unwrap(),
        b"()"
    );
    assert_eq!(data.root_count(), 0);
}

#[test]
fn aggregates_escape_text_preserve_bytes_and_sort_maps_after_moving_gc() {
    let mut heap = DomainHeap::new(802);
    let mut data = ManagedData::new(802);
    let values = vec![
        V::String,
        V::Bytes(5),
        V::Scalar(K::I64),
        V::Enum {
            identity: "Choice".into(),
            variants: vec![vec![], vec![0]],
        },
        V::List(3),
        V::Map { key: 0, value: 4 },
        V::Record {
            identity: "Packet".into(),
            fields: vec![0, 1, 5],
        },
    ];
    let schema = DisplaySchema(vec![
        D::String,
        D::Bytes,
        D::Scalar(K::I64),
        D::Enum {
            name: "Choice".into(),
            variants: vec![
                ("empty".into(), vec![]),
                ("text".into(), vec![("value".into(), 0)]),
            ],
        },
        D::List(3),
        D::Map { key: 0, value: 4 },
        D::Record {
            name: "Packet".into(),
            fields: vec![("text".into(), 0), ("bytes".into(), 1), ("items".into(), 5)],
        },
    ]);
    data.configure_values(&heap, Arc::new(ValueSchema::new(values).unwrap()))
        .unwrap();
    let text = data.create_string(&mut heap, "日本語\0\n\"\\🙂").unwrap();
    let bytes = data
        .create_bytes(&mut heap, 5, &[0, 255, b'"', b'\\', b'A'])
        .unwrap();
    let empty = data.create_composite(&mut heap, 3, Some(0), &[]).unwrap();
    let choice = data
        .create_composite(&mut heap, 3, Some(1), &[S::Reference(text)])
        .unwrap();
    let list = data
        .create_composite(
            &mut heap,
            4,
            None,
            &[S::Reference(empty), S::Reference(choice)],
        )
        .unwrap();
    let z = data.create_string(&mut heap, "z").unwrap();
    let a = data.create_string(&mut heap, "a").unwrap();
    let map = data
        .create_composite(
            &mut heap,
            5,
            None,
            &[
                S::Reference(z),
                S::Reference(list),
                S::Reference(a),
                S::Reference(list),
            ],
        )
        .unwrap();
    let record = data
        .create_composite(
            &mut heap,
            6,
            None,
            &[S::Reference(text), S::Reference(bytes), S::Reference(map)],
        )
        .unwrap();
    data.retain_roots(&mut heap, [record, text]).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    let roots = data.root_count();
    let expected = r#"Packet(text: "日本語\u{00}\n\"\\🙂", bytes: b"\x00\xff\"\\A", items: {"a": [Choice.empty, Choice.text(value: "日本語\u{00}\n\"\\🙂")], "z": [Choice.empty, Choice.text(value: "日本語\u{00}\n\"\\🙂")]})"#;
    assert_eq!(
        data.display(&heap, &schema, 6, S::Reference(record))
            .unwrap(),
        expected.as_bytes()
    );
    assert_eq!(
        data.display(&heap, &schema, 0, S::Reference(text)).unwrap(),
        "日本語\0\n\"\\🙂".as_bytes()
    );
    assert_eq!(data.root_count(), roots);
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}

#[test]
fn twenty_thousand_levels_use_iterative_walk_and_release_all_resources() {
    let mut heap = DomainHeap::new(803);
    let mut data = ManagedData::new(803);
    let schema = DisplaySchema(vec![
        D::Record {
            name: "Node".into(),
            fields: vec![("children".into(), 1)],
        },
        D::List(0),
    ]);
    data.configure_values(
        &heap,
        Arc::new(
            ValueSchema::new(vec![
                V::Record {
                    identity: "Node".into(),
                    fields: vec![1],
                },
                V::List(0),
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    let mut child = data.create_composite(&mut heap, 1, None, &[]).unwrap();
    for _ in 0..20_000 {
        let record = data
            .create_composite(&mut heap, 0, None, &[S::Reference(child)])
            .unwrap();
        let list = data
            .create_composite(&mut heap, 1, None, &[S::Reference(record)])
            .unwrap();
        data.release(&mut heap, child).unwrap();
        data.release(&mut heap, record).unwrap();
        child = list;
    }
    heap.collect(CollectionKind::Full).unwrap();
    let text = data
        .display(&heap, &schema, 1, S::Reference(child))
        .unwrap();
    assert_eq!(text.len(), 20_000 * "[Node(children: )]".len() + 2);
    assert!(text.starts_with(b"[Node(children: [Node(children: "));
    assert_eq!(data.root_count(), 1);
    data.release_all(&mut heap).unwrap();
    heap.collect(CollectionKind::Full).unwrap();
    assert_eq!(heap.object_count(), 0);
}
