use rinzu_bindgen::source_package::*;

#[test]
fn accelerated_sha256_preserves_standard_digest_vectors() {
    assert_eq!(
        sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256(&vec![b'a'; 1_000_000]),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn canonical_record_layouts_share_only_when_every_abi_property_matches() {
    let base = TypeRecord {
        identity: "shared::Output".into(),
        canonical: "shared::Output".into(),
        c_type: "struct First".into(),
        size: "16".into(),
        alignment: "8".into(),
        bytes_length: None,
        fields: vec![FieldLayout {
            name: "value".into(),
            c_type: "int64_t".into(),
            offset: "8".into(),
        }],
    };
    let mut alias = base.clone();
    alias.c_type = "struct Second".into();
    assert_eq!(
        matching_record_layout([&base, &alias].into_iter(), &base.identity).unwrap(),
        &base
    );
    assert!(matching_record_layout([&base].into_iter(), "missing").is_err());
    for index in 0..8 {
        let mut changed = alias.clone();
        match index {
            0 => changed.canonical = "different".into(),
            1 => changed.size = "24".into(),
            2 => changed.alignment = "16".into(),
            3 => changed.bytes_length = Some("16".into()),
            4 => changed.fields[0].name = "other".into(),
            5 => changed.fields[0].c_type = "double".into(),
            6 => changed.fields[0].offset = "0".into(),
            _ => changed.fields.clear(),
        }
        assert!(
            matching_record_layout([&base, &changed].into_iter(), &base.identity)
                .unwrap_err()
                .contains("conflicting native record layout")
        );
    }
}

fn interface() -> SourceInterface {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/source-package/interface.json"
    ))
    .unwrap()
}

#[test]
fn repeated_validation_keeps_results_isolated_and_rejects_changed_bytes() {
    let generated = generate_source_package(interface(), b"cache-isolation").unwrap();
    let mut first = validate_source_descriptor(&generated.wire).unwrap();
    first.interface.package.package = "modified-by-caller".into();
    first.interface.functions.clear();
    assert_eq!(
        validate_source_descriptor(&generated.wire).unwrap(),
        generated.descriptor
    );
    let mut changed = generated.descriptor.clone();
    changed.glue_identity = "invalid".into();
    assert!(validate_source_descriptor(&serde_json::to_vec(&changed).unwrap()).is_err());
}

#[test]
fn public_templates_are_closed_and_change_source_identity() {
    let base = generate_source_package(interface(), b"native").unwrap();
    let mut value = interface();
    value.source_templates.insert(
        "generic::id".into(),
        SourceTemplateRecord {
            owner_type_parameters: Vec::new(),
            private_fields: Vec::new(),
            kind: "function".into(),
            type_parameters: vec![("T".into(), vec!["Equality".into()])],
            signature: "fn(T)->T".into(),
            const_fields: vec![],
            fields: vec![],
            variants: vec![],
        },
    );
    value
        .public_bindings
        .insert("generic.id".into(), "generic::id".into());
    let template = generate_source_package(value.clone(), b"native").unwrap();
    assert_eq!(
        (template.descriptor.major, template.descriptor.minor),
        (2, 5)
    );
    assert_ne!(base.descriptor.import_keys, template.descriptor.import_keys);
    value
        .source_templates
        .get_mut("generic::id")
        .unwrap()
        .type_parameters[0]
        .1 = vec!["UserMarker".into()];
    assert!(generate_source_package(value, b"native").is_err());
    let mut old: serde_json::Value = serde_json::from_slice(&template.wire).unwrap();
    old["minor"] = 1.into();
    assert!(validate_source_descriptor(&serde_json::to_vec(&old).unwrap()).is_err());
    old["minor"] = 2.into();
    assert!(validate_source_descriptor(&serde_json::to_vec(&old).unwrap()).is_err());
    let mut value = interface();
    value
        .public_bindings
        .insert("public.missing".into(), "missing".into());
    assert!(generate_source_package(value, b"native").is_err());
}

#[test]
fn generic_error_templates_are_readonly_and_distinguish_optional_constructors() {
    let mut model = interface();
    let fields = vec![
        ("name".into(), "String".into()),
        ("message".into(), "String".into()),
        ("source".into(), "Option<$rinzu_SourceLocation>".into()),
        ("cause".into(), "Option<Error>".into()),
        ("payload".into(), "Option<T>".into()),
    ];
    let schema = SourceTemplateRecord {
        owner_type_parameters: Vec::new(),
        private_fields: Vec::new(),
        kind: "error".into(),
        type_parameters: vec![("T".into(), vec![])],
        signature: "error(payload?:T)".into(),
        const_fields: vec![0, 1, 2, 3, 4],
        fields,
        variants: vec![],
    };
    model
        .source_templates
        .insert("generic::Failed".into(), schema.clone());
    model
        .public_bindings
        .insert("generic.Failed".into(), "generic::Failed".into());
    let optional = generate_source_package(model.clone(), b"native").unwrap();
    model
        .source_templates
        .get_mut("generic::Failed")
        .unwrap()
        .signature = "error(payload:Option<T>)".into();
    let required = generate_source_package(model.clone(), b"native").unwrap();
    assert_ne!(
        optional.descriptor.semantic_digest,
        required.descriptor.semantic_digest
    );
    for mutation in 0..5 {
        let mut invalid = schema.clone();
        match mutation {
            0 => {
                invalid.const_fields.pop();
            }
            1 => invalid.fields[4].0 = "other".into(),
            2 => invalid.fields[2].1 = "Option<Int>".into(),
            3 => invalid.signature = "error(payload?:String)".into(),
            _ => invalid.variants.push(("Unexpected".into(), vec![])),
        }
        model
            .source_templates
            .insert("generic::Failed".into(), invalid);
        assert!(generate_source_package(model.clone(), b"native").is_err());
    }
}

#[test]
fn canonical_keys_and_separate_artifact_digest() {
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let generated = generate_source_package(interface(), b"native input").unwrap();
    assert_eq!(
        validate_source_descriptor(&generated.wire).unwrap(),
        generated.descriptor
    );
    let changed_bytes = generate_source_package(interface(), b"other input").unwrap();
    assert_eq!(
        generated.descriptor.import_keys,
        changed_bytes.descriptor.import_keys
    );
    assert_eq!(
        generated.descriptor.semantic_digest,
        changed_bytes.descriptor.semantic_digest
    );
    assert_ne!(
        generated.descriptor.native_artifact_digest,
        changed_bytes.descriptor.native_artifact_digest
    );
    let mut flags = interface();
    flags
        .package
        .build_inputs
        .insert("flags".into(), "-O3".into());
    assert_ne!(
        generated.descriptor.semantic_digest,
        generate_source_package(flags, b"native input")
            .unwrap()
            .descriptor
            .semantic_digest
    );
    for modify in [
        |i: &mut SourceInterface| i.functions[0].effects.push("requires-owner".into()),
        |i: &mut SourceInterface| {
            i.functions[0].c_prototype = "int64_t add(int32_t a, int32_t b)".into()
        },
        |i: &mut SourceInterface| i.provided_header_digest = Some(sha256(b"header")),
        |i: &mut SourceInterface| i.types[0].canonical = "Int64Alias".into(),
    ] {
        let mut changed = interface();
        modify(&mut changed);
        assert_ne!(
            generated.descriptor.import_keys,
            generate_source_package(changed, b"native input")
                .unwrap()
                .descriptor
                .import_keys
        );
    }
}

#[test]
fn canonical_order_and_decimal_precision() {
    let mut value = interface();
    let mut second = value.functions[0].clone();
    second.identity = "spike.math.other".into();
    second.symbol = Some("other".into());
    second.c_prototype = "int64_t other(int64_t a, int64_t b)".into();
    value.functions.push(second);
    let a = generate_source_package(value.clone(), b"").unwrap();
    value.functions.reverse();
    let b = generate_source_package(value, b"").unwrap();
    assert_eq!(a.wire, b.wire);
    assert!(
        String::from_utf8(a.wire)
            .unwrap()
            .contains("\"18446744073709551615\"")
    );
}

#[test]
fn constant_types_are_closed_exact_and_part_of_import_identity() {
    let mut value = interface();
    value.constants.insert("narrow".into(), "127".into());
    value.constant_types.insert("narrow".into(), "CInt8".into());
    let original = generate_source_package(value.clone(), b"native").unwrap();
    value
        .constant_types
        .insert("narrow".into(), "CInt16".into());
    let changed = generate_source_package(value.clone(), b"native").unwrap();
    assert_ne!(
        original.descriptor.import_keys,
        changed.descriptor.import_keys
    );
    assert_ne!(
        original.descriptor.semantic_digest,
        changed.descriptor.semantic_digest
    );
    for (scalar, literal) in [
        ("CInt8", "128"),
        ("CUInt64", "-1"),
        ("Bool", "1"),
        ("Float", "1"),
        ("CSize", "1"),
        ("Int", "9223372036854775808"),
        ("CUInt64", "18446744073709551616"),
    ] {
        value.constant_types.insert("narrow".into(), scalar.into());
        value.constants.insert("narrow".into(), literal.into());
        assert!(
            generate_source_package(value.clone(), b"native").is_err(),
            "accepted {scalar} {literal}"
        );
    }
    value.constant_types.remove("narrow");
    assert!(generate_source_package(value.clone(), b"native").is_err());
    value.constants.remove("narrow");
    value.constant_types.insert("extra".into(), "Int".into());
    assert!(generate_source_package(value, b"native").is_err());
    let mut wire: serde_json::Value = serde_json::from_slice(&original.wire).unwrap();
    wire["interface"]
        .as_object_mut()
        .unwrap()
        .remove("constant_types");
    assert!(validate_source_descriptor(&serde_json::to_vec(&wire).unwrap()).is_err());
}

#[test]
fn rejects_unknown_and_noncanonical_wire() {
    let generated = generate_source_package(interface(), b"").unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&generated.wire).unwrap();
    value["interface"]["backend"]["unknownRequired"] = 1.into();
    assert!(validate_source_descriptor(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut wire = generated.wire.clone();
    wire.push(b' ');
    assert!(validate_source_descriptor(&wire).is_err());
}

#[test]
fn rejects_unimplemented_registration_guarantees_before_generating_an_image() {
    let mut model = interface();
    let function = &mut model.functions[0];
    function.source_type = "native(fn(Int)->Int)->Int".into();
    function.c_prototype = "int64_t add(int64_t (*handler)(int64_t))".into();
    function.callbacks.push(CallbackRecord {
        source_type: "fn(Int)->Int".into(),
        foreign_to_main: false,
        lifetime: "current-native-call".into(),
        gate: "admission-close-settle-v1".into(),
        origin: "actual-caller-domain".into(),
        parent_scope: "actual-caller-scope".into(),
        target: model.package.target.clone(),
        code_version: 1,
        fallback: None,
    });
    assert!(generate_source_package(model.clone(), b"native").is_ok());
    for change in [
        |callback: &mut CallbackRecord| callback.lifetime = "registration".into(),
        |callback: &mut CallbackRecord| callback.gate = "epoch-only".into(),
        |callback: &mut CallbackRecord| callback.parent_scope = "detached".into(),
        |callback: &mut CallbackRecord| callback.origin = "foreign-heap".into(),
        |callback: &mut CallbackRecord| callback.foreign_to_main = true,
        |callback: &mut CallbackRecord| callback.fallback = Some("assumed-safe".into()),
    ] {
        let mut changed = model.clone();
        change(&mut changed.functions[0].callbacks[0]);
        let error = generate_source_package(changed, b"native").err().unwrap();
        assert!(error.contains("current-native-call wrapper"), "{error}");
    }
}

#[test]
fn generated_table_compiles_and_contains_only_v2_entry() {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return;
    }
    let generated = generate_source_package(interface(), b"input").unwrap();
    let dir = std::env::temp_dir().join(format!("rinzu-source-generation-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("source-package.h"), generated.header).unwrap();
    std::fs::write(dir.join("table.c"), generated.c_table).unwrap();
    std::fs::write(
        dir.join("math.c"),
        "#include <stdint.h>\nint64_t add(int64_t a,int64_t b){return a+b;}\n",
    )
    .unwrap();
    let include = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include");
    let result = std::process::Command::new("clang")
        .args(["-Wall", "-Wextra", "-Werror", "-dynamiclib", "-I"])
        .arg(include)
        .arg(dir.join("table.c"))
        .arg(dir.join("math.c"))
        .arg("-o")
        .arg(dir.join("package.dylib"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let symbols = std::process::Command::new("nm")
        .arg("-gU")
        .arg(dir.join("package.dylib"))
        .output()
        .unwrap();
    let symbols = String::from_utf8(symbols.stdout).unwrap();
    assert!(symbols.contains("_rinzu_source_package_v2"));
    assert!(!symbols.contains("_rinzu_native_package_v1"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn managed_code_fallback_and_public_aliases_are_canonical_without_native_entries() {
    let mut model = interface();
    let mut managed = model.functions[0].clone();
    managed.identity = "spike.math.identity".to_owned();
    managed.kind = FunctionKind::Sync;
    managed.source_type = "fn(Int)->Int".to_owned();
    managed.symbol = None;
    managed.c_prototype.clear();
    managed.effects = vec!["worker-safe".to_owned(), "cleanup-safe".to_owned()];
    managed.code = Some(CodeRecord {
        declaration: managed.identity.clone(),
        version: 1,
        fallback: Some(FallbackRecord {
            kind: "integer".to_owned(),
            scalar: Some("Int".to_owned()),
            value: "-1".to_owned(),
            bytes_length: None,
        }),
    });
    model.functions.push(managed);
    model.public_bindings.insert(
        "spike.math.alias".to_owned(),
        "spike.math.identity".to_owned(),
    );
    let generated = generate_source_package(model.clone(), b"native").unwrap();
    assert_eq!(generated.descriptor.import_keys.len(), 2);
    assert!(!generated.header.contains("identity("));
    assert_eq!(generated.c_table.matches("(const void *)&").count(), 1);
    model.functions[1]
        .code
        .as_mut()
        .unwrap()
        .fallback
        .as_mut()
        .unwrap()
        .value = "-2".to_owned();
    let changed = generate_source_package(model.clone(), b"native").unwrap();
    assert_ne!(
        generated.descriptor.import_keys[1],
        changed.descriptor.import_keys[1]
    );
    assert_ne!(
        generated.descriptor.semantic_digest,
        changed.descriptor.semantic_digest
    );
    model.functions[1].symbol = Some("invalid".to_owned());
    assert!(generate_source_package(model, b"native").is_err());
}

#[test]
fn public_error_metadata_is_immutable_and_part_of_the_semantic_digest() {
    let mut model = interface();
    let schema = SourceTypeRecord {
        private_fields: Vec::new(),
        kind: "error".into(),
        const_fields: vec![0, 1, 2, 3, 4],
        fields: vec![
            ("name".into(), "String".into()),
            ("message".into(), "String".into()),
            ("source".into(), "Option<$rinzu_SourceLocation>".into()),
            ("cause".into(), "Option<Error>".into()),
            ("payload".into(), "Int".into()),
        ],
        variants: vec![],
    };
    model
        .source_types
        .insert("spike.math.Failed".into(), schema.clone());
    let original = generate_source_package(model.clone(), b"native").unwrap();
    model
        .source_types
        .get_mut("spike.math.Failed")
        .unwrap()
        .fields[4]
        .1 = "String".into();
    let changed = generate_source_package(model.clone(), b"native").unwrap();
    assert_ne!(
        original.descriptor.semantic_digest,
        changed.descriptor.semantic_digest
    );
    for mutation in 0..4 {
        let mut invalid = schema.clone();
        match mutation {
            0 => {
                invalid.const_fields.pop();
            }
            1 => {
                invalid.fields[0].1 = "Int".into();
            }
            2 => {
                invalid.fields.truncate(3);
                invalid.const_fields.truncate(3);
            }
            _ => {
                invalid.fields[4].0 = "other".into();
            }
        }
        model
            .source_types
            .insert("spike.math.Failed".into(), invalid);
        assert!(generate_source_package(model.clone(), b"native").is_err());
    }
}

#[test]
fn public_schema_changes_invalidate_semantics_and_keys() {
    let mut model = interface();
    model.source_types.insert(
        "spike.math.Packet".to_owned(),
        SourceTypeRecord {
            private_fields: Vec::new(),
            const_fields: vec![],
            kind: "record".to_owned(),
            fields: vec![("bytes".to_owned(), "Bytes(4)".to_owned())],
            variants: Vec::new(),
        },
    );
    let original = generate_source_package(model.clone(), b"native").unwrap();
    model
        .source_types
        .get_mut("spike.math.Packet")
        .unwrap()
        .fields[0]
        .1 = "Bytes(8)".to_owned();
    let changed = generate_source_package(model.clone(), b"native").unwrap();
    assert_ne!(
        original.descriptor.semantic_digest,
        changed.descriptor.semantic_digest
    );
    assert_ne!(
        original.descriptor.import_keys,
        changed.descriptor.import_keys
    );
    let fields = &mut model
        .source_types
        .get_mut("spike.math.Packet")
        .unwrap()
        .fields;
    fields.push(fields[0].clone());
    assert!(generate_source_package(model, b"native").is_err());
}

#[test]
fn callback_sites_preserve_exact_length_code_and_fallback_identity() {
    let mut model = interface();
    let native = &mut model.functions[0];
    native.source_type = "native(fn(Bytes(4))->CUInt64,Bytes(4))->CUInt64".into();
    native.parameter_requirements = vec!["fn(Bytes(4))->CUInt64".into(), "Bytes(4)".into()];
    native.c_prototype =
        "uint64_t add(uint64_t (*cb)(const unsigned char *), const unsigned char *data)".into();
    let mut body = native.clone();
    body.identity = "spike.math.measure".into();
    body.source_type = "fn(Bytes(4))->CUInt64".into();
    body.kind = FunctionKind::Sync;
    body.symbol = None;
    body.c_prototype.clear();
    body.code = Some(CodeRecord {
        declaration: body.identity.clone(),
        version: 1,
        fallback: None,
    });
    body.callback_sites = vec![CallbackSiteRecord {
        native_function: "spike.math.add".into(),
        parameter: 0,
        source_type: "fn(Bytes(4))->CUInt64".into(),
        capture: None,
        targets: vec![CodeRecord {
            declaration: "rinzu.core::byteLength:4".into(),
            version: 1,
            fallback: None,
        }],
    }];
    model.functions.push(body);
    let original = generate_source_package(model.clone(), b"native").unwrap();
    assert_eq!(
        validate_source_descriptor(&original.wire).unwrap(),
        original.descriptor
    );
    for mutation in 0..11 {
        let mut bad = model.clone();
        let site = &mut bad.functions[1].callback_sites[0];
        match mutation {
            0 => site.parameter = 1,
            1 => site.source_type = "fn(Bytes(8))->CUInt64".into(),
            2 => site.targets[0].declaration = "rinzu.core::byteLength:04".into(),
            3 => site.targets[0].declaration = "rinzu.core::byteLength:18446744073709551616".into(),
            4 => site.targets[0].declaration = "rinzu.core::utf8ByteLength".into(),
            5 => site.targets[0].version = 2,
            6 => site.targets.push(site.targets[0].clone()),
            7 => {
                site.targets[0].fallback = Some(FallbackRecord {
                    kind: "unit".into(),
                    scalar: None,
                    value: "".into(),
                    bytes_length: None,
                })
            }
            8 => bad.functions[1].parameter_requirements = vec!["function reference".into()],
            9 => site.native_function = "missing".into(),
            10 => site.targets[0].declaration = "other::missing".into(),
            _ => unreachable!(),
        }
        assert!(
            generate_source_package(bad, b"native").is_err(),
            "accepted mutation {mutation}"
        );
    }
    // A generic reference parameter has no closed local target until an application supplies one.
    let mut generic = model.clone();
    generic.functions[1].callback_sites[0].targets.clear();
    assert!(generate_source_package(generic, b"native").is_ok());
    let mut dependency = model.clone();
    dependency
        .package
        .build_inputs
        .insert("dependency:other".into(), sha256(b"checked interface"));
    dependency.functions[1].callback_sites[0].native_function = "other::invoke".into();
    dependency.functions[1].callback_sites[0].targets[0].declaration = "other::measure".into();
    assert!(generate_source_package(dependency, b"native").is_ok());
    let mut fallback_body = model.functions[1].clone();
    fallback_body.identity = "spike.math.four".into();
    fallback_body.callback_sites.clear();
    fallback_body.code = Some(CodeRecord {
        declaration: fallback_body.identity.clone(),
        version: 1,
        fallback: Some(FallbackRecord {
            kind: "integer".into(),
            scalar: Some("CUInt64".into()),
            value: "0".into(),
            bytes_length: None,
        }),
    });
    model.functions[1].callback_sites[0].targets = vec![fallback_body.code.clone().unwrap()];
    model.functions.push(fallback_body);
    let target_changed = generate_source_package(model.clone(), b"native").unwrap();
    assert_ne!(
        original.descriptor.semantic_digest,
        target_changed.descriptor.semantic_digest
    );
    assert_ne!(
        original.descriptor.import_keys[1],
        target_changed.descriptor.import_keys[2]
    );
    model.functions[2]
        .code
        .as_mut()
        .unwrap()
        .fallback
        .as_mut()
        .unwrap()
        .value = "1".into();
    assert!(
        generate_source_package(model, b"native").is_err(),
        "stale site fallback"
    );
}

#[test]
fn const_field_schema_changes_keys_and_rejects_invalid_indices() {
    let mut model = interface();
    model.source_types.insert(
        "spike.math.Mutable".into(),
        SourceTypeRecord {
            private_fields: Vec::new(),
            kind: "state".into(),
            const_fields: vec![],
            fields: vec![
                ("limit".into(), "Int".into()),
                ("value".into(), "Int".into()),
            ],
            variants: vec![],
        },
    );
    let original = generate_source_package(model.clone(), b"native").unwrap();
    assert_eq!(
        (original.descriptor.major, original.descriptor.minor),
        (2, 5)
    );
    model
        .source_types
        .get_mut("spike.math.Mutable")
        .unwrap()
        .const_fields
        .push(0);
    let changed = generate_source_package(model.clone(), b"native").unwrap();
    assert_ne!(
        original.descriptor.semantic_digest,
        changed.descriptor.semantic_digest
    );
    assert_ne!(
        original.descriptor.import_keys,
        changed.descriptor.import_keys
    );
    for fields in [vec![2], vec![0, 0]] {
        model
            .source_types
            .get_mut("spike.math.Mutable")
            .unwrap()
            .const_fields = fields;
        assert!(generate_source_package(model.clone(), b"native").is_err());
    }
    let mut old = changed.descriptor;
    old.minor = 0;
    assert!(validate_source_descriptor(&serde_json::to_vec(&old).unwrap()).is_err());
}

#[test]
fn native_data_contract_plans_distinguish_positions_and_reject_stale_wire() {
    let mut model = interface();
    for (name, kind, fields) in [
        ("Packet", "native-record", vec![("x".into(), "Int".into())]),
        ("Handle", "opaque", vec![]),
        ("Context", "record", vec![("value".into(), "Int".into())]),
    ] {
        model.source_types.insert(
            name.into(),
            SourceTypeRecord {
                private_fields: Vec::new(),
                kind: kind.into(),
                fields,
                const_fields: vec![],
                variants: vec![],
            },
        );
    }
    model.functions[0].source_type =
        "native(Packet,Packet,Handle,UserData<Context>,fn(Packet,UserData<Context>)->Int)->Packet"
            .into();
    let generated = generate_source_package(model, b"native").unwrap();
    let plan = &generated.descriptor.interface.functions[0].conversion_plan;
    assert_eq!(
        plan,
        &[
            "arg[0]:native-record-loan-writeback-v1:Packet",
            "arg[1]:native-record-loan-writeback-v1:Packet",
            "arg[2]:opaque-reference-domain-v1:Handle",
            "arg[3]:context-token-input-v1:UserData<Context>",
            "arg[4]:callback-current-call-v1:fn(Packet,UserData<Context>)->Int",
            "arg[4].callback.arg[0]:native-record-restore-or-copy-v1:Packet",
            "arg[4].callback.arg[1]:context-token-current-call-argument-v1:UserData<Context>",
            "arg[4].callback.result:scalar-v1:Int",
            "result:native-record-restore-or-copy-v1:Packet",
        ]
    );
    for mutation in 0..6 {
        let mut bad = generated.descriptor.clone();
        match mutation {
            0 => bad.minor = 3,
            1 => bad.interface.backend.native_data_contract = 0,
            2 => bad.interface.functions[0].conversion_plan = vec!["identity-pointer".into()],
            3 => bad.interface.functions[0].conversion_plan.swap(0, 1),
            4 => {
                bad.interface.functions[0].conversion_plan[0] =
                    "arg[0]:opaque-reference-domain-v1:Packet".into()
            }
            5 => bad.interface.functions[0]
                .conversion_plan
                .pop()
                .map(|_| ())
                .unwrap(),
            _ => unreachable!(),
        }
        // Rehashing the interface cannot make an unknown or stale plan valid.
        bad.semantic_digest = sha256(&serde_json::to_vec(&bad.interface).unwrap());
        assert!(
            validate_source_descriptor(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "mutation {mutation}"
        );
    }
    let mut old = serde_json::to_value(&generated.descriptor).unwrap();
    old["interface"]["backend"]
        .as_object_mut()
        .unwrap()
        .remove("native_data_contract");
    assert!(validate_source_descriptor(&serde_json::to_vec(&old).unwrap()).is_err());
    let mut context_return = generated.descriptor.interface;
    context_return.functions[0].source_type = "native(UserData<Context>)->UserData<Context>".into();
    let returned = generate_source_package(context_return.clone(), b"native").unwrap();
    assert_eq!(
        returned.descriptor.interface.functions[0].conversion_plan[1],
        "result:context-token-current-call-result-v1:UserData<Context>"
    );
    context_return.functions[0].source_type = "native(fn()->UserData<Context>)->Int".into();
    assert!(generate_source_package(context_return.clone(), b"native").is_err());
    context_return.functions[0].source_type = "native(fn()->Packet)->Int".into();
    assert!(generate_source_package(context_return, b"native").is_err());
}
