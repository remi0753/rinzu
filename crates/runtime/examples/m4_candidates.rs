//! Isolated representation/protocol candidates, never production free authority.
#[path = "support/m4_observe.rs"]
mod observe;
use observe::measure;
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::transport::{TransportBudget, TransportDirection, typed::Descriptor};
use rinzu_runtime::value::{Value, ValueSchema, ValueType};
use std::hint::black_box;
use std::sync::Arc;

// Matches the actual decoded HeapValue size while retaining malformed-tag
// cases. The source is a model of the packed integer representation, not a
// borrowed view into a product Domain or authority to cross a GC/native edge.
#[derive(Clone, Copy)]
enum Field {
    Null,
    Integer(i64),
    Reference([u64; 2]),
}
fn decode(words: &[u64]) -> Result<Field, ()> {
    match words[0] {
        0 => Ok(Field::Null),
        1 => Ok(Field::Integer(words[1] as i64)),
        2 => Ok(Field::Reference([1, words[1]])),
        _ => Err(()),
    }
}
fn integer(field: Field) -> Result<usize, ()> {
    match field {
        Field::Integer(value) => usize::try_from(value).map_err(|_| ()),
        Field::Reference(words) => {
            black_box(words);
            Err(())
        }
        Field::Null => Err(()),
    }
}
fn decode_bytes(
    mut fields: impl ExactSizeIterator<Item = Result<Field, ()>>,
) -> Result<Vec<u8>, ()> {
    let length = integer(fields.next().ok_or(())??)?;
    if fields.len() != length.div_ceil(8) {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(length);
    for field in fields {
        let word = match field? {
            Field::Integer(value) => (value as u64).to_le_bytes(),
            _ => return Err(()),
        };
        let count = (length - bytes.len()).min(8);
        if word[count..].iter().any(|byte| *byte != 0) {
            return Err(());
        }
        bytes.extend_from_slice(&word[..count]);
    }
    Ok(bytes)
}
fn expanded(words: &[u64]) -> Result<Vec<u8>, ()> {
    let mut fields = Vec::with_capacity(words.len() / 2);
    for words in words.chunks_exact(2) {
        fields.push(decode(words)?);
    }
    decode_bytes(fields.into_iter().map(Ok))
}
fn viewed(words: &[u64]) -> Result<Vec<u8>, ()> {
    decode_bytes(words.chunks_exact(2).map(decode))
}
fn small(words: &[u64]) -> Result<Vec<u8>, ()> {
    let length = integer(decode(words.get(..2).ok_or(())?)?)?;
    if length > 64 {
        return viewed(words);
    }
    if words.len() != 2 * (1 + length.div_ceil(8)) {
        return Err(());
    }
    let mut bytes = [0_u8; 64];
    for (index, field) in words[2..].chunks_exact(2).enumerate() {
        let word = match decode(field)? {
            Field::Integer(value) => (value as u64).to_le_bytes(),
            _ => return Err(()),
        };
        let count = (length - index * 8).min(8);
        if word[count..].iter().any(|byte| *byte != 0) {
            return Err(());
        }
        bytes[index * 8..index * 8 + count].copy_from_slice(&word[..count]);
    }
    Ok(bytes[..length].to_vec())
}
fn packed(bytes: &[u8]) -> Vec<u64> {
    let mut words = vec![1, bytes.len() as u64];
    for part in bytes.chunks(8) {
        let mut word = [0_u8; 8];
        word[..part.len()].copy_from_slice(part);
        words.extend_from_slice(&[1, u64::from_le_bytes(word)]);
    }
    words
}
fn main() {
    let repeats = 256;
    assert_eq!(
        size_of::<Field>(),
        size_of::<rinzu_runtime::heap::HeapValue>()
    );
    for bytes in [b"".as_slice(), b"a\0b", "日本語🙂".as_bytes(), &[255; 17]] {
        let words = packed(bytes);
        assert_eq!(expanded(&words).unwrap(), bytes);
        assert_eq!(viewed(&words).unwrap(), bytes);
        assert_eq!(small(&words).unwrap(), bytes);
    }
    let mut invalid = packed(b"x");
    invalid[2] = 9;
    let mut bad_padding = packed(b"x");
    bad_padding[3] |= 255 << 8;
    for words in [vec![1, u64::MAX], vec![1, 9, 1, 0], invalid, bad_padding] {
        assert!(expanded(&words).is_err() && viewed(&words).is_err() && small(&words).is_err());
    }
    let scalar = ScalarValue::from_bits(ScalarKind::I64, 42).unwrap();
    let mut rows = vec![
        measure("hypothetical-boxed-scalar", 8, repeats * 256, || {
            black_box(Box::new(scalar));
        }),
        measure("current-inline-scalar", 8, repeats * 256, || {
            black_box(scalar);
        }),
    ];
    for size in [0, 8, 1024, 65536] {
        let bytes = vec![7; size];
        let words = packed(&bytes);
        for (name, method) in [
            (
                "expanded-fields",
                expanded as fn(&[u64]) -> Result<Vec<u8>, ()>,
            ),
            ("borrowed-fields", viewed),
            ("small-stack", small),
        ] {
            rows.push(measure(name, size, repeats, || {
                black_box(method(black_box(&words)).unwrap());
            }));
        }
    }
    let backing: Arc<[u8]> = vec![7; 8 * 1024 * 1024].into();
    let ordinary = vec![7_u8; 65536];
    rows.push(measure("owned-backing-copy", 65536, repeats, || {
        black_box(ordinary.clone());
    }));
    rows.push(measure("shared-backing-view", 65536, repeats, || {
        black_box((Arc::clone(&backing), 0, 65536));
    }));
    let tiny = (Arc::clone(&backing), 0, 1);
    drop(backing);
    assert_eq!(tiny.0[tiny.1..tiny.1 + tiny.2], [7]);
    let retained_backing = tiny.0.len();
    let schema = Arc::new(ValueSchema::new(vec![ValueType::Scalar(ScalarKind::I64)]).unwrap());
    let one = Descriptor::new(schema.clone(), vec![0]).unwrap();
    let bulk = Descriptor::new(schema, vec![0; 128]).unwrap();
    let values = vec![Value::Scalar(scalar); 128];
    let budget = TransportBudget::new(1024, 1024);
    rows.push(measure("128-single-scalar-packets", 1024, repeats, || {
        for value in &values {
            let packet = one
                .encode(
                    std::slice::from_ref(value),
                    budget.reserve(TransportDirection::Input, 8).unwrap(),
                )
                .unwrap();
            black_box(packet.decode_typed(&one).unwrap());
        }
    }));
    rows.push(measure("one-128-scalar-packet", 1024, repeats, || {
        let packet = bulk
            .encode(
                &values,
                budget.reserve(TransportDirection::Input, 1024).unwrap(),
            )
            .unwrap();
        black_box(packet.decode_typed(&bulk).unwrap());
    }));
    assert_eq!(budget.snapshot().used_bytes, 0);
    rows.push(measure("128-individual-temporaries", 8192, repeats, || {
        let buffers = (0..128).map(|_| vec![7_u8; 64]).collect::<Vec<_>>();
        assert_eq!(
            buffers.iter().map(|bytes| bytes[0] as usize).sum::<usize>(),
            896
        );
        black_box(buffers);
    }));
    rows.push(measure("one-region-arena", 8192, repeats, || {
        let arena = vec![7_u8; 8192];
        assert_eq!(
            arena
                .chunks_exact(64)
                .map(|bytes| bytes[0] as usize)
                .sum::<usize>(),
            896
        );
        black_box(arena);
    }));
    let list = vec![0_u64; 256];
    rows.push(measure("1024-immutable-updates", 2048, 32, || {
        let mut current = list.clone();
        for index in 0..1024 {
            let mut next = current.clone();
            next[index % 256] = index as u64;
            current = next;
        }
        assert_eq!(current[255], 1023);
        black_box(current);
    }));
    rows.push(measure("1024-builder-updates", 2048, 32, || {
        let mut current = list.clone();
        for index in 0..1024 {
            current[index % 256] = index as u64;
        }
        assert_eq!(current[255], 1023);
        black_box(current);
    }));
    assert!(list.iter().all(|value| *value == 0));
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"scope":"isolated candidates, packed-field model plus actual typed packet implementation; no production adoption", "warmup":8,
        "small_slice_bytes":1,"small_slice_retained_backing_bytes":retained_backing,
        "normal_and_negative_vectors":8,"rows":rows})).unwrap());
}
