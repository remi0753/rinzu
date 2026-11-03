use rinzu_runtime::key::{KeyError, KeySchema, KeyType, KeyValue};
use rinzu_runtime::scalar::{ScalarKind as K, ScalarValue as V};
use std::process::Command;

fn integer(kind: K, value: i128) -> KeyValue {
    KeyValue::Integer(V::from_integer(kind, value).unwrap())
}
fn string(value: &str) -> KeyValue {
    KeyValue::String(value.to_owned())
}
fn variant(identity: &str, selected: u32, fields: Vec<KeyValue>) -> KeyValue {
    KeyValue::Enum {
        identity: identity.to_owned(),
        variant: selected,
        fields,
    }
}

fn emit_types(program: &mut String, types: &[KeyType], name: &str) {
    for (index, ty) in types.iter().enumerate() {
        if let KeyType::Record { fields, .. } = ty
            && !fields.is_empty()
        {
            program.push_str(&format!(
                "static const size_t {name}_{index}_fields[] = {{{}}};\n",
                fields
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if let KeyType::Enum { variants, .. } = ty {
            for (v, fields) in variants.iter().enumerate() {
                if !fields.is_empty() {
                    program.push_str(&format!(
                        "static const size_t {name}_{index}_{v}[] = {{{}}};\n",
                        fields
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
            if !variants.is_empty() {
                program.push_str(&format!(
                    "static const rinzu_key_variant_t {name}_{index}_variants[] = {{"
                ));
                for (v, fields) in variants.iter().enumerate() {
                    program.push_str(&format!(
                        "{{{}, {}}},",
                        fields.len(),
                        if fields.is_empty() {
                            "NULL".to_owned()
                        } else {
                            format!("{name}_{index}_{v}")
                        }
                    ));
                }
                program.push_str("};\n");
            }
        }
    }
    program.push_str(&format!("static const rinzu_key_type_t {name}[] = {{"));
    for (index, ty) in types.iter().enumerate() {
        match ty {
            KeyType::Integer(kind) => program.push_str(&format!(
                "{{.kind=RINZU_KEY_INTEGER,.scalar_kind={}}},",
                *kind as u32
            )),
            KeyType::String => program.push_str("{.kind=RINZU_KEY_STRING},"),
            KeyType::Bool => program.push_str("{.kind=RINZU_KEY_BOOL},"),
            KeyType::Bytes { length } => program.push_str(&format!(
                "{{.kind=RINZU_KEY_BYTES,.bytes_fixed={},.bytes_length=UINT64_C({})}},",
                u8::from(length.is_some()),
                length.unwrap_or(0)
            )),
            KeyType::Record { identity, fields } => program.push_str(&format!(
                "{{.kind=RINZU_KEY_RECORD,.identity=\"{identity}\",.field_count={},.fields={}}},",
                fields.len(),
                if fields.is_empty() {
                    "NULL".to_owned()
                } else {
                    format!("{name}_{index}_fields")
                }
            )),
            KeyType::NotKey => program.push_str("{.kind=RINZU_KEY_NOT_KEY},"),
            KeyType::Enum { identity, variants } => program.push_str(&format!(
                "{{.kind=RINZU_KEY_ENUM,.identity=\"{identity}\",.variant_count={},.variants={}}},",
                variants.len(),
                if variants.is_empty() {
                    "NULL".to_owned()
                } else {
                    format!("{name}_{index}_variants")
                }
            )),
        }
    }
    program.push_str("};\n");
}

fn emit_value(program: &mut String, value: &KeyValue, index: &mut usize) -> String {
    let name = format!("value{}", *index);
    *index += 1;
    match value {
        KeyValue::Integer(value) => program.push_str(&format!("const rinzu_key_value_t {name} = {{.kind=RINZU_KEY_INTEGER,.scalar_kind={},.bits=UINT64_C({})}};\n", value.kind() as u32, value.bits())),
        KeyValue::Bool(value) => program.push_str(&format!("const rinzu_key_value_t {name} = {{.kind=RINZU_KEY_BOOL,.scalar_kind=RINZU_SCALAR_BOOL,.bits={}}};\n", u8::from(*value))),
        KeyValue::Bytes(bytes) => {
            let content = if bytes.is_empty() { "0".to_owned() } else { bytes.iter().map(u8::to_string).collect::<Vec<_>>().join(",") };
            program.push_str(&format!("const unsigned char {name}_bytes[] = {{{content}}}; const rinzu_key_value_t {name} = {{.kind=RINZU_KEY_BYTES,.bytes={name}_bytes,.length={}}};\n", bytes.len()));
        }
        KeyValue::Record { identity, fields } => {
            let children = fields.iter().map(|field| emit_value(program, field, index)).collect::<Vec<_>>();
            if !children.is_empty() { program.push_str(&format!("const rinzu_key_value_t {name}_fields[] = {{{}}};\n", children.join(","))); }
            program.push_str(&format!("const rinzu_key_value_t {name} = {{.kind=RINZU_KEY_RECORD,.identity=\"{identity}\",.count={},.fields={}}};\n", fields.len(), if fields.is_empty() { "NULL".to_owned() } else { format!("{name}_fields") }));
        }
        KeyValue::String(text) => {
            let bytes = if text.is_empty() { "0".to_owned() } else { text.as_bytes().iter().map(u8::to_string).collect::<Vec<_>>().join(",") };
            program.push_str(&format!("const unsigned char {name}_bytes[] = {{{bytes}}}; const rinzu_key_value_t {name} = {{.kind=RINZU_KEY_STRING,.bytes={name}_bytes,.length={}}};\n", text.len()));
        }
        KeyValue::Enum { identity, variant, fields } => {
            let children = fields.iter().map(|field| emit_value(program, field, index)).collect::<Vec<_>>();
            if !children.is_empty() { program.push_str(&format!("const rinzu_key_value_t {name}_fields[] = {{{}}};\n", children.join(","))); }
            program.push_str(&format!("const rinzu_key_value_t {name} = {{.kind=RINZU_KEY_ENUM,.identity=\"{identity}\",.variant={variant},.count={},.fields={}}};\n", fields.len(), if fields.is_empty() { "NULL".to_owned() } else { format!("{name}_fields") }));
        }
    }
    name
}

#[test]
fn closed_enum_keys_use_all_variants_nominal_identity_and_typed_payloads_in_rust_and_c() {
    let types = vec![
        KeyType::Integer(K::U64),
        KeyType::String,
        KeyType::Integer(K::I8),
        KeyType::Enum {
            identity: "pkg.Inner".into(),
            variants: vec![vec![], vec![0, 1]],
        },
        KeyType::Enum {
            identity: "pkg.Outer".into(),
            variants: vec![vec![], vec![1, 3]],
        },
        KeyType::Enum {
            identity: "dependency.Other".into(),
            variants: vec![vec![], vec![1, 3]],
        },
        KeyType::Integer(K::Bool),
        KeyType::Enum {
            identity: "pkg.UnusedBool".into(),
            variants: vec![vec![], vec![6]],
        },
        KeyType::Enum {
            identity: "pkg.CycleA".into(),
            variants: vec![vec![9]],
        },
        KeyType::Enum {
            identity: "pkg.CycleB".into(),
            variants: vec![vec![8]],
        },
        KeyType::NotKey,
        KeyType::Enum {
            identity: "pkg.UnusedRecord".into(),
            variants: vec![vec![], vec![10]],
        },
        KeyType::Enum {
            identity: "pkg.Wide".into(),
            variants: vec![vec![0; 129]],
        },
    ];
    let schema = KeySchema::new(types.clone()).unwrap();
    let cases = [
        variant("pkg.Outer", 0, vec![]),
        variant(
            "pkg.Outer",
            1,
            vec![string("日本語\0tail"), variant("pkg.Inner", 0, vec![])],
        ),
        variant(
            "pkg.Outer",
            1,
            vec![
                string("日本語\0tail"),
                variant(
                    "pkg.Inner",
                    1,
                    vec![integer(K::U64, u64::MAX as i128), string("e\u{301}")],
                ),
            ],
        ),
        variant(
            "pkg.Outer",
            1,
            vec![
                string("日本語\0tail"),
                variant(
                    "pkg.Inner",
                    1,
                    vec![integer(K::U64, u64::MAX as i128), string("é")],
                ),
            ],
        ),
        variant(
            "pkg.Outer",
            1,
            vec![
                string("other"),
                variant("pkg.Inner", 1, vec![integer(K::U64, 0), string("")]),
            ],
        ),
    ];
    let root = std::env::temp_dir().join(format!("rinzu-enum-key-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut program =
        String::from("#include <assert.h>\n#include <stdio.h>\n#include \"rinzu/enum_key.h\"\n");
    emit_types(&mut program, &types, "types");
    // A large, acyclic type/value graph has no fixed template nesting limit.
    let mut deep_types = (0..512)
        .map(|index| {
            if index % 2 == 0 {
                KeyType::Record {
                    identity: format!("deep.E{index}"),
                    fields: vec![index + 1],
                }
            } else {
                KeyType::Enum {
                    identity: format!("deep.E{index}"),
                    variants: vec![vec![index + 1]],
                }
            }
        })
        .collect::<Vec<_>>();
    deep_types.push(KeyType::Integer(K::U64));
    let deep_schema = KeySchema::new(deep_types.clone()).unwrap();
    let mut deep = integer(K::U64, u64::MAX as i128);
    for index in (0..512).rev() {
        deep = if index % 2 == 0 {
            KeyValue::Record {
                identity: format!("deep.E{index}"),
                fields: vec![deep],
            }
        } else {
            variant(&format!("deep.E{index}"), 0, vec![deep])
        };
    }
    emit_types(&mut program, &deep_types, "deep_types");
    program.push_str("int main(void) { rinzu_key_schema_t schema={0}; assert(!rinzu_key_schema_init(&schema,types,sizeof(types)/sizeof(*types))); assert(rinzu_key_schema_init(&schema,types,sizeof(types)/sizeof(*types)) == RINZU_KEY_SCHEMA);\n");
    program.push_str(r#"
  { rinzu_key_schema_t untouched={0}; const size_t invalid[]={99};
    const rinzu_key_variant_t variants[]={{1,invalid}};
    const rinzu_key_type_t bad[]={{.kind=RINZU_KEY_ENUM,.identity="bad",.variant_count=1,.variants=variants}};
    assert(rinzu_key_schema_init(&untouched,bad,1)==RINZU_KEY_SCHEMA);
    const rinzu_key_type_t duplicate[]={{.kind=RINZU_KEY_ENUM,.identity="same"},{.kind=RINZU_KEY_ENUM,.identity="same"}};
    assert(rinzu_key_schema_init(&untouched,duplicate,2)==RINZU_KEY_SCHEMA);
    assert(!untouched.types && !untouched.allowed && !untouched.count);
  }
  { const rinzu_key_value_t bad={.kind=RINZU_KEY_INTEGER,.scalar_kind=RINZU_SCALAR_I8,.bits=255}; uint64_t hash=91;
    assert(rinzu_key_hash(&schema,2,&bad,&hash)==RINZU_KEY_VALUE && hash==91);
    const rinzu_key_value_t null_text={.kind=RINZU_KEY_STRING,.length=1};
    assert(rinzu_key_hash(&schema,1,&null_text,&hash)==RINZU_KEY_VALUE && hash==91);
  }
"#);
    for index in 0..types.len() {
        program.push_str(&format!(
            "assert(schema.allowed[{index}] == {});\n",
            u8::from(schema.is_key(index).unwrap())
        ));
    }
    let mut index = 0;
    let names = cases
        .iter()
        .map(|value| emit_value(&mut program, value, &mut index))
        .collect::<Vec<_>>();
    let mut expected = Vec::new();
    for (value, name) in cases.iter().zip(&names) {
        let copy = emit_value(&mut program, &value.clone(), &mut index);
        expected.push(schema.hash(4, value).unwrap());
        assert!(schema.equal(4, value, &value.clone()).unwrap());
        program.push_str(&format!("{{ uint64_t hash=0,copy=0; int equal=0; assert(!rinzu_key_hash(&schema,4,&{name},&hash)); assert(!rinzu_key_hash(&schema,4,&{copy},&copy)); assert(hash==copy); assert(!rinzu_key_equal(&schema,4,&{name},&{copy},&equal) && equal); printf(\"%llu\\n\",(unsigned long long)hash); }}\n"));
    }
    for (left, a) in cases.iter().zip(&names) {
        for (right, b) in cases.iter().zip(&names) {
            expected.push(u64::from(schema.equal(4, left, right).unwrap()));
            program.push_str(&format!("{{ int equal=37; assert(!rinzu_key_equal(&schema,4,&{a},&{b},&equal)); printf(\"%d\\n\",equal); }}\n"));
        }
    }
    let other = variant("dependency.Other", 0, vec![]);
    assert_ne!(
        schema.hash(4, &cases[0]).unwrap(),
        schema.hash(5, &other).unwrap()
    );
    assert_eq!(schema.hash(4, &other), Err(KeyError::TypeMismatch));
    let other_name = emit_value(&mut program, &other, &mut index);
    expected.push(schema.hash(5, &other).unwrap());
    program.push_str(&format!("{{ uint64_t hash=0; assert(!rinzu_key_hash(&schema,5,&{other_name},&hash)); printf(\"%llu\\n\",(unsigned long long)hash); char identity[] = \"pkg.Outer\"; rinzu_key_value_t copy={}; copy.identity=identity; uint64_t nominal_copy=0; assert(!rinzu_key_hash(&schema,4,&copy,&nominal_copy)); uint64_t original=0; assert(!rinzu_key_hash(&schema,4,&{},&original)); assert(nominal_copy==original); }}\n", names[0], names[0]));
    for (type_id, value, error) in [
        (7, variant("pkg.UnusedBool", 0, vec![]), KeyError::NotKey),
        (11, variant("pkg.UnusedRecord", 0, vec![]), KeyError::NotKey),
        (4, variant("pkg.Outer", 2, vec![]), KeyError::TypeMismatch),
        (
            4,
            variant(
                "pkg.Outer",
                1,
                vec![
                    string("first differs"),
                    variant("pkg.Inner", 1, vec![integer(K::I64, 1), string("")]),
                ],
            ),
            KeyError::TypeMismatch,
        ),
        (4, other, KeyError::TypeMismatch),
    ] {
        assert_eq!(schema.hash(type_id, &value), Err(error));
        let name = emit_value(&mut program, &value, &mut index);
        let status = if error == KeyError::NotKey {
            "RINZU_KEY_EXCLUDED"
        } else {
            "RINZU_KEY_VALUE"
        };
        program.push_str(&format!("{{ uint64_t hash=91; assert(rinzu_key_hash(&schema,{type_id},&{name},&hash)=={status}); assert(hash==91); int equal=37; assert(rinzu_key_equal(&schema,{type_id},&{name},&{name},&equal)=={status}); assert(equal==37); }}\n"));
        if error == KeyError::TypeMismatch && type_id == 4 {
            assert_eq!(
                schema.equal(4, &cases[1], &value),
                Err(KeyError::TypeMismatch)
            );
            program.push_str(&format!("{{ int equal=37; assert(rinzu_key_equal(&schema,4,&{},&{name},&equal)==RINZU_KEY_VALUE); assert(equal==37); }}\n",names[1]));
        }
    }
    let wide = variant("pkg.Wide", 0, vec![integer(K::U64, u64::MAX as i128); 129]);
    let wide_name = emit_value(&mut program, &wide, &mut index);
    expected.push(schema.hash(12, &wide).unwrap());
    program.push_str(&format!("{{ uint64_t hash=0; int equal=0; assert(!rinzu_key_hash(&schema,12,&{wide_name},&hash)); assert(!rinzu_key_equal(&schema,12,&{wide_name},&{wide_name},&equal) && equal); printf(\"%llu\\n\",(unsigned long long)hash); }}\n"));
    let name = emit_value(&mut program, &deep, &mut index);
    expected.push(deep_schema.hash(0, &deep).unwrap());
    program.push_str(&format!("rinzu_key_schema_t deep_schema={{0}}; assert(!rinzu_key_schema_init(&deep_schema,deep_types,sizeof(deep_types)/sizeof(*deep_types))); {{ uint64_t hash=0; int equal=0; assert(!rinzu_key_hash(&deep_schema,0,&{name},&hash)); assert(!rinzu_key_equal(&deep_schema,0,&{name},&{name},&equal) && equal); printf(\"%llu\\n\",(unsigned long long)hash); }} rinzu_key_schema_dispose(&deep_schema); rinzu_key_schema_dispose(&schema); assert(!schema.allowed && !schema.types && !schema.count); return 0; }}\n"));
    std::fs::write(root.join("test.c"), program).unwrap();
    let compile = Command::new("clang")
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(root.join("test.c"))
        .arg("-o")
        .arg(root.join("test"))
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(root.join("test")).output().unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let actual = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|line| line.parse::<u64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    let sanitized = Command::new("clang")
        .args([
            "-std=c11",
            "-O1",
            "-g",
            "-fsanitize=address,undefined",
            "-I",
        ])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(root.join("test.c"))
        .arg("-o")
        .arg(root.join("sanitized"))
        .output()
        .unwrap();
    assert!(
        sanitized.status.success(),
        "{}",
        String::from_utf8_lossy(&sanitized.stderr)
    );
    let sanitized = Command::new(root.join("sanitized")).output().unwrap();
    assert!(
        sanitized.status.success(),
        "{}",
        String::from_utf8_lossy(&sanitized.stderr)
    );
    assert_eq!(
        String::from_utf8(sanitized.stdout)
            .unwrap()
            .lines()
            .map(|line| line.parse::<u64>().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn adopted_bool_bytes_and_records_match_candidate_hashes_and_validate_complete_inputs() {
    let types = vec![
        KeyType::Bool,
        KeyType::Bytes { length: None },
        KeyType::Bytes { length: Some(4) },
        KeyType::Record {
            identity: "Inner".into(),
            fields: vec![0, 1],
        },
        KeyType::Record {
            identity: "Outer".into(),
            fields: vec![3, 2, 0],
        },
        KeyType::Record {
            identity: "Other".into(),
            fields: vec![0, 1],
        },
        KeyType::Record {
            identity: "Empty".into(),
            fields: vec![],
        },
        KeyType::NotKey,
        KeyType::Record {
            identity: "Bad".into(),
            fields: vec![7],
        },
        KeyType::Enum {
            identity: "BadEnum".into(),
            variants: vec![vec![], vec![8]],
        },
        KeyType::Record {
            identity: "Cycle".into(),
            fields: vec![10],
        },
    ];
    let schema = KeySchema::new(types.clone()).unwrap();
    let inner = KeyValue::Record {
        identity: "Inner".into(),
        fields: vec![KeyValue::Bool(true), KeyValue::Bytes(vec![0, 255])],
    };
    let fixed = KeyValue::Bytes(vec![65, 0, 0, 0]);
    let cases = vec![
        (0, KeyValue::Bool(false), 605032694565748564),
        (0, KeyValue::Bool(true), 605033794077376775),
        (1, KeyValue::Bytes(vec![]), 16523567575176999244),
        (1, KeyValue::Bytes(vec![0, 255, 128]), 5317305400058572658),
        (2, fixed.clone(), 13455358537592189337),
        (3, inner.clone(), 12209657619712400817),
        (
            4,
            KeyValue::Record {
                identity: "Outer".into(),
                fields: vec![inner, fixed, KeyValue::Bool(false)],
            },
            1795943501115571834,
        ),
        (
            5,
            KeyValue::Record {
                identity: "Other".into(),
                fields: vec![KeyValue::Bool(true), KeyValue::Bytes(vec![0, 255])],
            },
            8812493430779637639,
        ),
    ];
    let mut program =
        String::from("#include <assert.h>\n#include <stdio.h>\n#include \"rinzu/enum_key.h\"\n");
    emit_types(&mut program, &types, "types");
    program.push_str("int main(void) {rinzu_key_schema_t schema={0};assert(!rinzu_key_schema_init(&schema,types,sizeof(types)/sizeof(*types)));\n");
    for root in 0..types.len() {
        assert_eq!(schema.is_key(root).unwrap(), root < 7);
        program.push_str(&format!(
            "assert(schema.allowed[{root}]=={});\n",
            u8::from(root < 7)
        ));
    }
    let mut next = 0;
    for (root, value, hash) in &cases {
        assert_eq!(schema.hash(*root, value).unwrap(), *hash);
        assert!(schema.equal(*root, value, &value.clone()).unwrap());
        let name = emit_value(&mut program, value, &mut next);
        let copy = emit_value(&mut program, &value.clone(), &mut next);
        program.push_str(&format!("{{uint64_t hash=0,copy=0;int equal=0;assert(!rinzu_key_hash(&schema,{root},&{name},&hash));assert(!rinzu_key_hash(&schema,{root},&{copy},&copy));assert(hash==UINT64_C({hash}) && copy==hash);assert(!rinzu_key_equal(&schema,{root},&{name},&{copy},&equal) && equal);}}\n"));
    }
    for (root, value, error) in [
        (2, KeyValue::Bytes(vec![65]), KeyError::TypeMismatch),
        (
            3,
            KeyValue::Record {
                identity: "Other".into(),
                fields: vec![],
            },
            KeyError::TypeMismatch,
        ),
        (
            3,
            KeyValue::Record {
                identity: "Inner".into(),
                fields: vec![KeyValue::Bool(false), KeyValue::Bool(true)],
            },
            KeyError::TypeMismatch,
        ),
        (9, variant("BadEnum", 0, vec![]), KeyError::NotKey),
    ] {
        assert_eq!(schema.hash(root, &value), Err(error));
        assert_eq!(schema.equal(root, &value, &value), Err(error));
        let name = emit_value(&mut program, &value, &mut next);
        let error = if error == KeyError::NotKey {
            "RINZU_KEY_EXCLUDED"
        } else {
            "RINZU_KEY_VALUE"
        };
        program.push_str(&format!("{{uint64_t hash=91;int equal=37;assert(rinzu_key_hash(&schema,{root},&{name},&hash)=={error} && hash==91);assert(rinzu_key_equal(&schema,{root},&{name},&{name},&equal)=={error} && equal==37);}}\n"));
    }
    assert!(
        !schema
            .equal(0, &KeyValue::Bool(false), &KeyValue::Bool(true))
            .unwrap()
    );
    assert!(
        !schema
            .equal(
                1,
                &KeyValue::Bytes(vec![0, 255]),
                &KeyValue::Bytes(vec![255, 0])
            )
            .unwrap()
    );
    program.push_str(r#"
      {uint64_t hash=91; const rinzu_key_value_t bad_bool={.kind=RINZU_KEY_BOOL,.scalar_kind=RINZU_SCALAR_BOOL,.bits=2};
       assert(rinzu_key_hash(&schema,0,&bad_bool,&hash)==RINZU_KEY_VALUE && hash==91);
       const rinzu_key_value_t bad_bytes={.kind=RINZU_KEY_BYTES,.length=1};
       assert(rinzu_key_hash(&schema,1,&bad_bytes,&hash)==RINZU_KEY_VALUE && hash==91);
       const rinzu_key_value_t empty={.kind=RINZU_KEY_RECORD,.identity="Empty"};
       int equal=0;assert(!rinzu_key_equal(&schema,6,&empty,&empty,&equal) && equal);
       const rinzu_key_type_t duplicate[]={{.kind=RINZU_KEY_RECORD,.identity="same"},{.kind=RINZU_KEY_ENUM,.identity="same"}};
       rinzu_key_schema_t untouched={0};assert(rinzu_key_schema_init(&untouched,duplicate,2)==RINZU_KEY_SCHEMA);}
      rinzu_key_schema_dispose(&schema);return 0;}
    "#);
    let directory = std::env::temp_dir().join(format!("rinzu-m1-key-c-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("test.c"), program).unwrap();
    for sanitized in [false, true] {
        let binary = directory.join(if sanitized { "sanitized" } else { "test" });
        let mut command = Command::new("clang");
        command
            .args(["-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror", "-I"])
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(directory.join("test.c"))
            .arg("-o")
            .arg(&binary);
        if sanitized {
            command.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
        }
        let compiled = command.output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn malformed_closed_key_schemas_are_rejected_before_publication() {
    assert!(matches!(
        KeySchema::new(vec![KeyType::Enum {
            identity: "bad".into(),
            variants: vec![vec![1]]
        }]),
        Err(KeyError::InvalidSchema)
    ));
    assert!(matches!(
        KeySchema::new(vec![
            KeyType::Enum {
                identity: "duplicate".into(),
                variants: vec![vec![]]
            },
            KeyType::Enum {
                identity: "duplicate".into(),
                variants: vec![vec![]]
            }
        ]),
        Err(KeyError::InvalidSchema)
    ));
    assert!(matches!(
        KeySchema::new(vec![KeyType::Enum {
            identity: "bad\0identity".into(),
            variants: vec![vec![]]
        }]),
        Err(KeyError::InvalidSchema)
    ));
}
