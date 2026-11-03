use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as S};
use rinzu_runtime::value::{Value as V, ValueError as E, ValueSchema, ValueType as T};
use std::process::Command;

#[path = "support/value_schema.rs"]
mod value_schema;
use value_schema::emit_types;

fn emit_value(program: &mut String, value: &V, next: &mut usize) -> String {
    let mut names = std::collections::HashMap::new();
    let mut stack = vec![(value, false)];
    while let Some((current, ready)) = stack.pop() {
        if !ready {
            stack.push((current, true));
            match current {
                V::Record { fields, .. } | V::Enum { fields, .. } | V::List(fields) => {
                    stack.extend(fields.iter().rev().map(|value| (value, false)))
                }
                V::Map(entries) => {
                    for (key, value) in entries.iter().rev() {
                        stack.push((value, false));
                        stack.push((key, false));
                    }
                }
                _ => {}
            }
        } else {
            let name = emit_one(program, current, next, &names);
            names.insert(current as *const V, name);
        }
    }
    names.remove(&(value as *const V)).unwrap()
}

fn emit_one(
    program: &mut String,
    value: &V,
    next: &mut usize,
    names: &std::collections::HashMap<*const V, String>,
) -> String {
    let name = format!("v{}", *next);
    *next += 1;
    let initializer = match value {
        V::Scalar(s) => format!(
            ".kind=RINZU_VALUE_SCALAR,.scalar_kind={},.bits=UINT64_C({})",
            s.kind() as u32,
            s.bits()
        ),
        V::Unit => ".kind=RINZU_VALUE_UNIT".into(),
        V::Error(_) => panic!("Error has no equality in these vectors"),
        V::String(text) => {
            let bytes = if text.is_empty() {
                "0".into()
            } else {
                text.as_bytes()
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            };
            program.push_str(&format!(
                "const unsigned char {name}_bytes[]={{{bytes}}};\n"
            ));
            format!(
                ".kind=RINZU_VALUE_STRING,.bytes={name}_bytes,.length={}",
                text.len()
            )
        }
        V::Bytes(bytes) => {
            let values = if bytes.is_empty() {
                "0".into()
            } else {
                bytes
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            };
            program.push_str(&format!(
                "const unsigned char {name}_bytes[]={{{values}}};\n"
            ));
            format!(
                ".kind=RINZU_VALUE_BYTES,.bytes={name}_bytes,.length={}",
                bytes.len()
            )
        }
        V::Record { fields, .. } | V::Enum { fields, .. } | V::List(fields) => {
            let children = fields
                .iter()
                .map(|field| names[&(field as *const V)].clone())
                .collect::<Vec<_>>();
            if !children.is_empty() {
                program.push_str(&format!(
                    "const rinzu_value_t {name}_fields[]={{{}}};\n",
                    children.join(",")
                ));
            }
            let prefix = match value {
                V::Record { identity, .. } => {
                    format!(".kind=RINZU_VALUE_RECORD,.identity=\"{identity}\"")
                }
                V::Enum {
                    identity, variant, ..
                } => format!(".kind=RINZU_VALUE_ENUM,.identity=\"{identity}\",.variant={variant}"),
                _ => ".kind=RINZU_VALUE_LIST".into(),
            };
            format!(
                "{prefix},.count={},.fields={}",
                fields.len(),
                if fields.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_fields")
                }
            )
        }
        V::Map(entries) => {
            let children = entries
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{{{}, {}}}",
                        names[&(key as *const V)],
                        names[&(value as *const V)]
                    )
                })
                .collect::<Vec<_>>();
            if !children.is_empty() {
                program.push_str(&format!(
                    "const rinzu_value_pair_t {name}_entries[]={{{}}};\n",
                    children.join(",")
                ));
            }
            format!(
                ".kind=RINZU_VALUE_MAP,.count={},.entries={}",
                entries.len(),
                if entries.is_empty() {
                    "NULL".into()
                } else {
                    format!("{name}_entries")
                }
            )
        }
        V::Other { ty, slot } => {
            format!(".kind=RINZU_VALUE_OTHER,.other_type={ty},.bits=UINT64_C({slot})")
        }
    };
    program.push_str(&format!("const rinzu_value_t {name}={{{initializer}}};\n"));
    name
}
fn text(value: &str) -> V {
    V::String(value.into())
}
fn scalar(value: u64) -> V {
    V::Scalar(S::from_bits(K::U64, value).unwrap())
}
fn record(float: f64) -> V {
    V::Record {
        identity: "pkg.Record".into(),
        fields: vec![
            V::Scalar(S::from_bits(K::Bool, 1).unwrap()),
            V::Bytes(vec![0, 1, 255]),
            V::Scalar(S::from_f64(float)),
        ],
    }
}
fn status(error: E) -> u32 {
    match error {
        E::InvalidSchema => 1,
        E::TypeMismatch => 2,
        E::NoEquality => 3,
        E::NotKey => 4,
        E::DuplicateKey => 5,
    }
}

#[test]
fn aot_composite_validation_equality_and_map_settlement_match_rust() {
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
            identity: "pkg.Unused".into(),
            variants: vec![vec![], vec![21]],
        },
        T::Record {
            identity: "pkg.ContentKey".into(),
            fields: vec![26, 14, 27, 10],
        },
        T::Map { key: 24, value: 7 },
        T::Record {
            identity: "pkg.ContentInner".into(),
            fields: vec![10, 14],
        },
        T::Enum {
            identity: "pkg.Single".into(),
            variants: vec![vec![], vec![14]],
        },
    ]);
    let schema = ValueSchema::new(types.clone()).unwrap();
    let mut program =
        String::from("#include <assert.h>\n#include <stdio.h>\n#include \"rinzu/value.h\"\n");
    emit_types(&mut program, &types, "types");
    let mut deep_types = (0..512).map(|index| T::List(index + 1)).collect::<Vec<_>>();
    deep_types.push(T::Scalar(K::U64));
    emit_types(&mut program, &deep_types, "deep_types");
    let deep_schema = ValueSchema::new(deep_types).unwrap();
    let mut deep = scalar(u64::MAX);
    for _ in 0..512 {
        deep = V::List(vec![deep]);
    }
    let wide_types = vec![
        T::Scalar(K::U64),
        T::Record {
            identity: "wide".into(),
            fields: vec![0; 129],
        },
    ];
    emit_types(&mut program, &wide_types, "wide_types");
    let wide_schema = ValueSchema::new(wide_types).unwrap();
    let wide = V::Record {
        identity: "wide".into(),
        fields: vec![scalar(u64::MAX); 129],
    };
    program.push_str("int main(void) { rinzu_value_schema_t schema={0}; assert(!rinzu_value_schema_init(&schema,types,sizeof(types)/sizeof(*types)));\n");
    for index in 0..types.len() {
        program.push_str(&format!(
            "assert(schema.equality[{index}]=={} && schema.keys.allowed[{index}]=={});\n",
            u8::from(schema.has_equality(index).unwrap()),
            u8::from(schema.is_key(index).unwrap())
        ));
    }
    let plus = record(0.0);
    let minus = record(-0.0);
    let nan = record(f64::NAN);
    let first = schema
        .map(
            19,
            vec![
                (text("a"), V::List(vec![plus.clone()])),
                (text("b"), V::List(vec![record(2.0)])),
            ],
        )
        .unwrap();
    let reverse = schema
        .map(
            19,
            vec![
                (text("b"), V::List(vec![record(2.0)])),
                (text("a"), V::List(vec![minus.clone()])),
            ],
        )
        .unwrap();
    let map_nan = schema
        .map(19, vec![(text("a"), V::List(vec![nan.clone()]))])
        .unwrap();
    let key = V::Enum {
        identity: "pkg.Key".into(),
        variant: 1,
        fields: vec![scalar(u64::MAX), text("日本語\0tail")],
    };
    let duplicate = V::Map(vec![
        (key.clone(), scalar(0)),
        (key.clone(), scalar(u64::MAX)),
    ]);
    let invalid_late = V::Record {
        identity: "pkg.Record".into(),
        fields: vec![
            V::Scalar(S::from_bits(K::Bool, 0).unwrap()),
            V::Bytes(vec![0, 1, 255]),
            V::Scalar(S::from_f32(0.0)),
        ],
    };
    let mut cases = vec![
        (11, V::Unit, V::Unit),
        (13, V::Bytes(vec![]), V::Bytes(vec![])),
        (14, V::Bytes(vec![0, 1, 255]), V::Bytes(vec![0, 1, 255])),
        (14, V::Bytes(vec![0, 1]), V::Bytes(vec![0, 1])),
        (12, text("日本語\0tail"), text("日本語\0tail")),
        (12, text("é"), text("e\u{301}")),
        (15, plus.clone(), minus),
        (15, nan.clone(), nan.clone()),
        (15, plus.clone(), invalid_late),
        (
            18,
            V::List(vec![plus.clone(), nan.clone()]),
            V::List(vec![plus.clone(), nan]),
        ),
        (
            16,
            V::Enum {
                identity: "pkg.Payload".into(),
                variant: 1,
                fields: vec![plus, V::Scalar(S::from_f32(-0.0))],
            },
            V::Enum {
                identity: "pkg.Payload".into(),
                variant: 0,
                fields: vec![],
            },
        ),
        (19, first, reverse),
        (19, map_nan.clone(), map_nan),
        (20, duplicate.clone(), duplicate.clone()),
        (22, V::List(vec![]), V::List(vec![])),
        (
            23,
            V::Enum {
                identity: "pkg.Unused".into(),
                variant: 0,
                fields: vec![],
            },
            V::Enum {
                identity: "pkg.Unused".into(),
                variant: 0,
                fields: vec![],
            },
        ),
    ];
    for kind in (0..8).map(|tag| K::from_tag(tag).unwrap()) {
        let (minimum, maximum) = kind.bounds().unwrap();
        let min = V::Scalar(S::from_integer(kind, minimum).unwrap());
        let max = V::Scalar(S::from_integer(kind, maximum).unwrap());
        cases.push((kind as usize, max.clone(), max.clone()));
        cases.push((kind as usize, min, max));
    }
    for (root, value, zero, negative_zero) in [
        (
            8,
            V::Scalar(S::from_f32(f32::NAN)),
            V::Scalar(S::from_f32(0.0)),
            V::Scalar(S::from_f32(-0.0)),
        ),
        (
            9,
            V::Scalar(S::from_f64(f64::NAN)),
            V::Scalar(S::from_f64(0.0)),
            V::Scalar(S::from_f64(-0.0)),
        ),
    ] {
        cases.push((root, value.clone(), value));
        cases.push((root, zero, negative_zero));
    }
    let mut next = 0;
    let mut expected = Vec::new();
    for (root, left, right) in cases {
        let a = emit_value(&mut program, &left, &mut next);
        let b = emit_value(&mut program, &right, &mut next);
        let (code, equal) = match schema.equal(root, &left, &right) {
            Ok(value) => (0, i32::from(value)),
            Err(error) => (status(error), 37),
        };
        expected.push(format!("{code},{equal}"));
        program.push_str(&format!("{{ int equal=37; int status=rinzu_value_equal(&schema,{root},&{a},&{b},&equal); printf(\"%d,%d\\n\",status,equal); }}\n"));
    }
    let key_name = emit_value(&mut program, &key, &mut next);
    expected.push(format!("hash:{}", schema.key_hash(17, &key).unwrap()));
    program.push_str(&format!("{{ uint64_t hash=91; assert(!rinzu_value_key_hash(&schema,17,&{key_name},&hash)); printf(\"hash:%llu\\n\",(unsigned long long)hash); }}\n"));
    for (root, value) in [
        (10, V::Scalar(S::from_bits(K::Bool, 0).unwrap())),
        (10, V::Scalar(S::from_bits(K::Bool, 1).unwrap())),
        (13, V::Bytes(vec![])),
        (14, V::Bytes(vec![0, 255, 128])),
        (
            24,
            V::Record {
                identity: "pkg.ContentKey".into(),
                fields: vec![
                    V::Record {
                        identity: "pkg.ContentInner".into(),
                        fields: vec![V::Scalar(S::from_bool(true)), V::Bytes(vec![0, 255, 128])],
                    },
                    V::Bytes(vec![0, 255, 128]),
                    V::Enum {
                        identity: "pkg.Single".into(),
                        variant: 1,
                        fields: vec![V::Bytes(vec![0, 255, 128])],
                    },
                    V::Scalar(S::from_bits(K::Bool, 1).unwrap()),
                ],
            },
        ),
    ] {
        let name = emit_value(&mut program, &value, &mut next);
        let copy = emit_value(&mut program, &value.clone(), &mut next);
        expected.push(format!(
            "adopted:{}",
            schema.key_hash(root, &value).unwrap()
        ));
        program.push_str(&format!("{{uint64_t hash=0,copy=0;int equal=0;assert(!rinzu_value_key_hash(&schema,{root},&{name},&hash));assert(!rinzu_value_key_hash(&schema,{root},&{copy},&copy) && hash==copy);assert(!rinzu_value_equal(&schema,{root},&{name},&{copy},&equal) && equal);printf(\"adopted:%llu\\n\",(unsigned long long)hash);}}\n"));
        if root == 24 {
            let entries = vec![(value.clone(), scalar(1)), (value, scalar(2))];
            let raw = emit_value(&mut program, &V::Map(entries.clone()), &mut next);
            let normalized = schema.map(25, entries).unwrap();
            assert!(
                matches!(normalized,V::Map(entries) if entries.len()==1 && schema.equal(7,&entries[0].1,&scalar(2)).unwrap())
            );
            program.push_str(&format!("{{rinzu_value_map_t output={{0}};assert(!rinzu_value_map(&schema,25,{raw}.entries,{raw}.count,&output));assert(output.count==1 && output.entries[0].value.bits==2);rinzu_value_map_dispose(&output);}}\n"));
        }
    }
    let raw = emit_value(&mut program, &duplicate, &mut next);
    let normalized = schema
        .map(
            20,
            if let V::Map(entries) = duplicate {
                entries
            } else {
                unreachable!()
            },
        )
        .unwrap();
    let normalized_name = emit_value(&mut program, &normalized, &mut next);
    program.push_str(&format!("{{ rinzu_value_map_t map={{0}}; assert(!rinzu_value_map(&schema,20,{raw}.entries,{raw}.count,&map)); assert(map.count==1); rinzu_value_t value={{.kind=RINZU_VALUE_MAP,.count=map.count,.entries=map.entries}}; int equal=0; assert(!rinzu_value_equal(&schema,20,&value,&{normalized_name},&equal) && equal); assert(map.entries[0].value.bits==UINT64_MAX); rinzu_value_map_dispose(&map); assert(!map.entries && !map.count); }}\n"));
    program.push_str(r#"
  { const unsigned char bad[]={0xed,0xa0,0x80}; const rinzu_value_t text={.kind=RINZU_VALUE_STRING,.bytes=bad,.length=3};
    uint64_t hash=91; int equal=37; assert(rinzu_value_key_hash(&schema,12,&text,&hash)==RINZU_VALUE_TYPE && hash==91);
    assert(rinzu_value_equal(&schema,12,&text,&text,&equal)==RINZU_VALUE_TYPE && equal==37);
    rinzu_key_hasher_t state=rinzu_key_init(); uint64_t original=state.hash; assert(rinzu_key_string(&state,bad,3) && state.hash==original);
  }
  { rinzu_value_schema_t untouched={0}; const rinzu_value_type_t bad[]={ {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_F64}, {.kind=RINZU_VALUE_MAP,.key=0,.value=0} };
    assert(rinzu_value_schema_init(&untouched,bad,2)==RINZU_VALUE_SCHEMA); assert(!untouched.types && !untouched.equality && !untouched.key_types && !untouched.keys.allowed);
    const size_t missing[]={99}; const rinzu_value_type_t bad_field[]={ {.kind=RINZU_VALUE_RECORD,.identity="bad",.field_count=1,.fields=missing} };
    assert(rinzu_value_schema_init(&untouched,bad_field,1)==RINZU_VALUE_SCHEMA);
  }
"#);
    let bad = emit_value(
        &mut program,
        &V::Map(vec![
            (key.clone(), V::Scalar(S::from_integer(K::I64, 0).unwrap())),
            (key, scalar(1)),
        ]),
        &mut next,
    );
    program.push_str(&format!("{{ rinzu_value_map_t output={{0}}; assert(rinzu_value_map(&schema,20,{bad}.entries,{bad}.count,&output)==RINZU_VALUE_TYPE); assert(!output.entries && !output.count); }}\n"));
    for (name, types_name, root, value, schema) in [
        ("deep_schema", "deep_types", 0, &deep, &deep_schema),
        ("wide_schema", "wide_types", 1, &wide, &wide_schema),
    ] {
        assert!(schema.equal(root, value, &value.clone()).unwrap());
        let a = emit_value(&mut program, value, &mut next);
        program.push_str(&format!("{{ rinzu_value_schema_t {name}={{0}}; assert(!rinzu_value_schema_init(&{name},{types_name},sizeof({types_name})/sizeof(*{types_name}))); int equal=0; assert(!rinzu_value_equal(&{name},{root},&{a},&{a},&equal) && equal); rinzu_value_schema_dispose(&{name}); }}\n"));
    }
    program.push_str("rinzu_value_schema_dispose(&schema); assert(!schema.types && !schema.key_types && !schema.equality && !schema.keys.allowed); return 0; }\n");
    let directory = std::env::temp_dir().join(format!("rinzu-composite-c-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("test.c"), program).unwrap();
    for (name, flags) in [
        ("optimized", vec!["-O2", "-Wall", "-Wextra", "-Werror"]),
        (
            "sanitized",
            vec!["-O1", "-g", "-fsanitize=address,undefined"],
        ),
    ] {
        let compile = Command::new("clang")
            .arg("-std=c11")
            .args(flags)
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(directory.join("test.c"))
            .arg("-o")
            .arg(directory.join(name))
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(directory.join(name)).output().unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8(run.stdout)
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            expected
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
