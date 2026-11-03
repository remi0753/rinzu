use rinzu_runtime::key::{KeySchema, KeyType, KeyValue};
use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as S};
use rinzu_runtime::value::{Value as V, ValueError as E, ValueSchema, ValueType as T};

fn scalar(kind: K, value: i128) -> V {
    V::Scalar(S::from_integer(kind, value).unwrap())
}
fn text(value: &str) -> V {
    V::String(value.into())
}
fn enumeration(identity: &str, variant: u32, fields: Vec<V>) -> V {
    V::Enum {
        identity: identity.into(),
        variant,
        fields,
    }
}
fn record(float: f64) -> V {
    V::Record {
        identity: "pkg.Record".into(),
        fields: vec![
            V::Scalar(S::from_bits(K::Bool, 1).unwrap()),
            V::Bytes(vec![1, 0, 255]),
            V::Scalar(S::from_f64(float)),
        ],
    }
}
fn schema() -> ValueSchema {
    let mut types = (0..11)
        .map(|tag| T::Scalar(K::from_tag(tag).unwrap()))
        .collect::<Vec<_>>();
    types.extend([
        T::Unit,
        T::String,
        T::Bytes(0),
        T::Bytes(3),
        T::Record {
            identity: "pkg.Record".into(),
            fields: vec![10, 14, 9],
        },
        T::Enum {
            identity: "pkg.Payload".into(),
            variants: vec![vec![], vec![15, 8]],
        },
        T::Enum {
            identity: "pkg.Key".into(),
            variants: vec![vec![], vec![7, 12]],
        },
        T::List(15),
        T::Map { key: 12, value: 18 },
        T::Map { key: 17, value: 7 },
        T::Other,
        T::List(21),
        T::Enum {
            identity: "pkg.UnusedOther".into(),
            variants: vec![vec![], vec![21]],
        },
        T::Record {
            identity: "dependency.Record".into(),
            fields: vec![10, 14, 9],
        },
    ]);
    ValueSchema::new(types).unwrap()
}

#[test]
fn deep_equality_preserves_ieee_values_bytes_nominal_types_and_payload_order() {
    let schema = schema();
    assert!(schema.equal(11, &V::Unit, &V::Unit).unwrap());
    assert!(
        schema
            .equal(13, &V::Bytes(vec![]), &V::Bytes(vec![]))
            .unwrap()
    );
    assert!(
        schema
            .equal(14, &V::Bytes(vec![1, 0, 255]), &V::Bytes(vec![1, 0, 255]))
            .unwrap()
    );
    assert!(
        !schema
            .equal(14, &V::Bytes(vec![1, 0, 255]), &V::Bytes(vec![1, 255, 0]))
            .unwrap()
    );
    assert_eq!(
        schema.equal(14, &V::Bytes(vec![1, 0]), &V::Bytes(vec![1, 0])),
        Err(E::TypeMismatch)
    );
    assert!(
        schema
            .equal(12, &text("日本語\0tail"), &text("日本語\0tail"))
            .unwrap()
    );
    assert!(!schema.equal(12, &text("é"), &text("e\u{301}")).unwrap());
    for ty in [8, 9] {
        let value = if ty == 8 {
            |x| V::Scalar(S::from_f32(x as f32))
        } else {
            |x| V::Scalar(S::from_f64(x))
        };
        assert!(schema.equal(ty, &value(0.0), &value(-0.0)).unwrap());
        assert!(
            !schema
                .equal(ty, &value(f64::INFINITY), &value(f64::NEG_INFINITY))
                .unwrap()
        );
        let nan = value(f64::NAN);
        assert!(!schema.equal(ty, &nan, &nan).unwrap());
    }
    let plus = record(0.0);
    let minus = record(-0.0);
    let nan = record(f64::NAN);
    assert!(schema.equal(15, &plus, &minus).unwrap());
    assert!(!schema.equal(15, &nan, &nan).unwrap());
    let list = V::List(vec![plus.clone(), nan.clone()]);
    assert!(!schema.equal(18, &list, &list).unwrap());
    assert!(
        !schema
            .equal(
                18,
                &V::List(vec![plus.clone()]),
                &V::List(vec![plus.clone(), minus])
            )
            .unwrap()
    );
    let payload = enumeration(
        "pkg.Payload",
        1,
        vec![plus.clone(), V::Scalar(S::from_f32(-0.0))],
    );
    assert!(schema.equal(16, &payload, &payload.clone()).unwrap());
    assert!(
        !schema
            .equal(16, &payload, &enumeration("pkg.Payload", 0, vec![]))
            .unwrap()
    );
    let bad = V::Record {
        identity: "pkg.Record".into(),
        fields: vec![
            V::Scalar(S::from_bits(K::Bool, 0).unwrap()),
            V::Bytes(vec![1, 0, 255]),
            V::Scalar(S::from_f32(0.0)),
        ],
    };
    assert_eq!(schema.equal(15, &plus, &bad), Err(E::TypeMismatch));
    let foreign = V::Record {
        identity: "dependency.Record".into(),
        fields: vec![
            V::Scalar(S::from_bits(K::Bool, 1).unwrap()),
            V::Bytes(vec![1, 0, 255]),
            V::Scalar(S::from_f64(0.0)),
        ],
    };
    assert_eq!(schema.equal(15, &plus, &foreign), Err(E::TypeMismatch));
    assert!(schema.equal(24, &foreign, &foreign).unwrap());
}

#[test]
fn maps_coalesce_later_values_and_compare_pair_sets_without_identity_shortcuts() {
    let schema = schema();
    let first = schema
        .map(
            19,
            vec![
                (text("a"), V::List(vec![record(1.0)])),
                (text("b"), V::List(vec![record(2.0)])),
            ],
        )
        .unwrap();
    let reverse = schema
        .map(
            19,
            vec![
                (text("b"), V::List(vec![record(2.0)])),
                (text("a"), V::List(vec![record(1.0)])),
            ],
        )
        .unwrap();
    assert!(schema.equal(19, &first, &reverse).unwrap());
    let nan = schema
        .map(19, vec![(text("a"), V::List(vec![record(f64::NAN)]))])
        .unwrap();
    assert!(!schema.equal(19, &nan, &nan).unwrap());
    let key = enumeration(
        "pkg.Key",
        1,
        vec![scalar(K::U64, u64::MAX as i128), text("日本語\0tail")],
    );
    let before = schema
        .map(20, vec![(key.clone(), scalar(K::U64, 0))])
        .unwrap();
    let after = schema
        .map(
            20,
            vec![
                (key.clone(), scalar(K::U64, 0)),
                (key.clone(), scalar(K::U64, u64::MAX as i128)),
            ],
        )
        .unwrap();
    let expected = schema
        .map(20, vec![(key.clone(), scalar(K::U64, u64::MAX as i128))])
        .unwrap();
    assert!(!schema.equal(20, &before, &after).unwrap());
    assert!(schema.equal(20, &after, &expected).unwrap());
    let keys = KeySchema::new(vec![
        KeyType::Integer(K::U64),
        KeyType::String,
        KeyType::Enum {
            identity: "pkg.Key".into(),
            variants: vec![vec![], vec![0, 1]],
        },
    ])
    .unwrap();
    let key_value = KeyValue::Enum {
        identity: "pkg.Key".into(),
        variant: 1,
        fields: vec![
            KeyValue::Integer(S::from_integer(K::U64, u64::MAX as i128).unwrap()),
            KeyValue::String("日本語\0tail".into()),
        ],
    };
    assert_eq!(
        schema.key_hash(17, &key).unwrap(),
        keys.hash(2, &key_value).unwrap()
    );
    let duplicate = V::Map(vec![
        (key.clone(), scalar(K::U64, 0)),
        (key.clone(), scalar(K::U64, 1)),
    ]);
    assert_eq!(schema.validate(20, &duplicate), Err(E::DuplicateKey));
    assert_eq!(schema.equal(20, &before, &duplicate), Err(E::DuplicateKey));
    // An invalid earlier RHS is still rejected when a later entry overwrites it.
    assert!(matches!(
        schema.map(
            20,
            vec![
                (key.clone(), scalar(K::I64, 0)),
                (key.clone(), scalar(K::U64, 1))
            ]
        ),
        Err(E::TypeMismatch)
    ));
    let missing = schema
        .map(19, vec![(text("different"), V::List(vec![]))])
        .unwrap();
    let bad = V::Map(vec![(text("a"), scalar(K::U64, 0))]);
    assert_eq!(schema.equal(19, &missing, &bad), Err(E::TypeMismatch));
}

#[test]
fn non_key_types_and_uncomparable_aggregate_types_are_rejected_even_when_empty() {
    let schema = schema();
    for root in [8, 9, 11, 15, 16, 18, 19, 20, 21, 22, 23, 24] {
        assert!(!schema.is_key(root).unwrap(), "{root}");
    }
    for (root, value) in [(15, record(0.0)), (18, V::List(vec![]))] {
        assert_eq!(schema.key_hash(root, &value), Err(E::NotKey));
    }
    for (root, value) in [
        (
            21,
            V::Other {
                ty: 21,
                slot: u64::MAX,
            },
        ),
        (22, V::List(vec![])),
        (23, enumeration("pkg.UnusedOther", 0, vec![])),
    ] {
        assert!(!schema.has_equality(root).unwrap());
        assert_eq!(schema.equal(root, &value, &value), Err(E::NoEquality));
    }
    for forbidden in [
        T::Scalar(K::F32),
        T::Unit,
        T::Record {
            identity: "record".into(),
            fields: vec![3],
        },
        T::List(1),
        T::Other,
    ] {
        assert!(matches!(
            ValueSchema::new(vec![
                forbidden,
                T::Scalar(K::U64),
                T::Map { key: 0, value: 1 },
                T::Scalar(K::F64),
            ]),
            Err(E::InvalidSchema)
        ));
    }
    assert!(matches!(
        ValueSchema::new(vec![T::Record {
            identity: "bad".into(),
            fields: vec![1]
        }]),
        Err(E::InvalidSchema)
    ));
    assert!(matches!(
        ValueSchema::new(vec![
            T::Record {
                identity: "duplicate".into(),
                fields: vec![]
            },
            T::Enum {
                identity: "duplicate".into(),
                variants: vec![vec![]]
            }
        ]),
        Err(E::InvalidSchema)
    ));
}

#[test]
fn deep_and_wide_composite_graphs_compare_without_native_recursion_or_fixed_capacity() {
    let mut types = (0..512).map(|index| T::List(index + 1)).collect::<Vec<_>>();
    types.push(T::Scalar(K::U64));
    let schema = ValueSchema::new(types).unwrap();
    let mut value = scalar(K::U64, u64::MAX as i128);
    for _ in 0..512 {
        value = V::List(vec![value]);
    }
    assert!(schema.equal(0, &value, &value.clone()).unwrap());
    let schema = ValueSchema::new(vec![
        T::Scalar(K::U64),
        T::Record {
            identity: "wide".into(),
            fields: vec![0; 129],
        },
    ])
    .unwrap();
    let value = V::Record {
        identity: "wide".into(),
        fields: vec![scalar(K::U64, u64::MAX as i128); 129],
    };
    assert!(schema.equal(1, &value, &value.clone()).unwrap());
    let cycle = ValueSchema::new(vec![T::List(0)]).unwrap();
    assert!(!cycle.has_equality(0).unwrap());
    assert!(!cycle.is_key(0).unwrap());
}
