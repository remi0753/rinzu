use rinzu_bindgen::source_package::*;
fn model() -> SourceInterface {
    let mut model: SourceInterface = serde_json::from_str(include_str!(
        "../../../tests/fixtures/source-package/interface.json"
    ))
    .unwrap();
    model.source_types.insert(
        "spike.State".into(),
        SourceTypeRecord {
            const_fields: vec![],
            kind: "record".into(),
            fields: vec![("id".into(), "Int".into())],
            variants: vec![],
        },
    );
    model.types.push(TypeRecord {
        identity: "UserData<spike.State>".into(),
        canonical: "UserData<spike.State>".into(),
        c_type: "void *".into(),
        size: "8".into(),
        alignment: "8".into(),
        bytes_length: None,
        fields: vec![],
    });
    let native = &mut model.functions[0];
    native.source_type = "native(fn(UserData<spike.State>,String)->Int)->Int".into();
    native.parameter_requirements = vec!["fn(UserData<spike.State>,String)->Int".into()];
    native.c_prototype = "int64_t add(int64_t (*callback)(void *,const char *))".into();
    let mut body = native.clone();
    body.identity = "spike.handler".into();
    body.source_type = "fn(spike.State,String)->Int".into();
    body.kind = FunctionKind::Sync;
    body.symbol = None;
    body.c_prototype.clear();
    body.parameter_requirements = vec!["spike.State".into(), "String".into()];
    body.code = Some(CodeRecord {
        declaration: body.identity.clone(),
        version: 1,
        fallback: None,
    });
    body.callback_sites = vec![CallbackSiteRecord {
        native_function: native.identity.clone(),
        parameter: 0,
        source_type: native.parameter_requirements[0].clone(),
        capture: None,
        targets: vec![body.code.clone().unwrap()],
    }];
    model.functions.push(body);
    model
}
#[test]
fn managed_userdata_callback_keeps_nominal_function_type_and_exact_mapping() {
    let original = model();
    let generated = generate_source_package(original.clone(), b"native").unwrap();
    assert_eq!(
        validate_source_descriptor(&generated.wire).unwrap(),
        generated.descriptor
    );
    for mutation in 0..8 {
        let mut bad = original.clone();
        match mutation {
            0 => bad.types.retain(|ty| !ty.identity.starts_with("UserData<")),
            1 => bad.types.last_mut().unwrap().canonical = "UserData<wrong>".into(),
            2 => bad.types.last_mut().unwrap().c_type = "const char *".into(),
            3 => bad.source_types.clear(),
            4 => bad.source_types.get_mut("spike.State").unwrap().kind = "native-record".into(),
            5 => bad.functions[1].source_type = "fn(spike.Other,String)->Int".into(),
            6 => bad.functions[1].callback_sites[0].targets[0].version = 2,
            7 => bad.functions[1].source_type = "fn(spike.State,String)->CUInt64".into(),
            _ => unreachable!(),
        }
        assert!(
            generate_source_package(bad, b"native").is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn captured_callback_metadata_checks_the_hidden_managed_argument() {
    let mut original = model();
    original.source_types.insert(
        "spike.Capture".into(),
        SourceTypeRecord {
            kind: "state".into(),
            const_fields: vec![],
            fields: vec![("environment".into(), "spike.Environment".into())],
            variants: vec![],
        },
    );
    original.source_types.insert(
        "spike.Environment".into(),
        SourceTypeRecord {
            kind: "enum".into(),
            const_fields: vec![],
            fields: vec![],
            variants: vec![("one".into(), vec![])],
        },
    );
    let body = &mut original.functions[1];
    body.source_type = "fn(spike.Capture,spike.State,String)->Int".into();
    body.parameter_requirements
        .insert(0, "spike.Capture".into());
    body.callback_sites[0].capture = Some("spike.Capture".into());
    let generated = generate_source_package(original.clone(), b"native").unwrap();
    assert_eq!(
        validate_source_descriptor(&generated.wire).unwrap(),
        generated.descriptor
    );
    for mutation in 0..8 {
        let mut bad = original.clone();
        match mutation {
            0 => bad.functions[1].callback_sites[0].capture = None,
            1 => {
                bad.source_types.remove("spike.Capture");
            }
            2 => bad.source_types.get_mut("spike.Capture").unwrap().kind = "record".into(),
            3 => bad.source_types.get_mut("spike.Environment").unwrap().kind = "record".into(),
            4 => bad.functions[1].source_type = "fn(spike.State,String)->Int".into(),
            5 => {
                bad.functions[1].parameter_requirements.remove(0);
            }
            6 => {
                bad.functions[1].callback_sites[0].targets[0].declaration =
                    bad.functions[0].identity.clone()
            }
            7 => {
                bad.functions[1].callback_sites[0].targets[0].declaration =
                    "dependency::missing".into()
            }
            _ => unreachable!(),
        }
        assert!(
            generate_source_package(bad, b"native").is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn supplant_callback_sites_validate_the_native_parameter_contract() {
    let mut original = model();
    original.functions[0].kind = FunctionKind::Supplant;
    original.functions[0].source_type =
        original.functions[0]
            .source_type
            .replacen("native(", "supplant(", 1);
    let generated = generate_source_package(original.clone(), b"native").unwrap();
    validate_source_descriptor(&generated.wire).unwrap();
    for kind in [FunctionKind::Sync, FunctionKind::Async] {
        let mut invalid = original.clone();
        invalid.functions[0].kind = kind;
        assert!(generate_source_package(invalid, b"native").is_err());
    }
    original.functions[1].callback_sites[0].parameter = 1;
    assert!(generate_source_package(original, b"native").is_err());
}
#[test]
fn raw_native_callback_preserves_physical_userdata_abi_and_has_no_managed_fallback() {
    let mut model = model();
    let mut raw = model.functions[0].clone();
    raw.identity = "spike.raw".into();
    raw.symbol = Some("raw".into());
    raw.source_type = "native(UserData<spike.State>,String)->Int".into();
    raw.parameter_requirements = vec!["UserData<spike.State>".into(), "String".into()];
    raw.c_prototype = "int64_t raw(void *data,const char *text)".into();
    raw.callback_sites.clear();
    raw.code = None;
    model.functions[1].callback_sites[0].targets = vec![CodeRecord {
        declaration: raw.identity.clone(),
        version: 1,
        fallback: None,
    }];
    model.functions.push(raw);
    assert!(generate_source_package(model.clone(), b"native").is_ok());
    model.functions[1].callback_sites[0].targets[0].fallback = Some(FallbackRecord {
        kind: "integer".into(),
        scalar: Some("Int".into()),
        value: "-1".into(),
        bytes_length: None,
    });
    assert!(generate_source_package(model, b"native").is_err());
}
