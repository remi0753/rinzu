use rinzu_compiler::{FileId, SourceFile, parse_source, validate_native_declarations};

fn validate(source: &str) -> bool {
    let file = SourceFile::new(FileId(0), "native.rz", source);
    parse_source(&file).is_ok_and(|syntax| validate_native_declarations(&syntax).is_ok())
}

#[test]
fn opaque_declarations_are_memberless_and_preserve_their_source_span() {
    use rinzu_compiler::{ast::NativeMember, format_source};
    let text = "// 外部参照\nnative { opaque Handle; }\n";
    let file = SourceFile::new(FileId(7), "opaque.rinzu", text);
    let syntax = parse_source(&file).unwrap();
    let NativeMember::Opaque { name, range } = &syntax.native_blocks[0].members[0] else {
        panic!("expected an opaque declaration");
    };
    assert_eq!(name, "Handle");
    assert_eq!(range.file_id, FileId(7));
    assert_eq!(
        &text[range.start_byte as usize..range.end_byte as usize],
        "opaque Handle;"
    );
    let formatted = format_source(&file).unwrap();
    assert!(formatted.contains("    opaque Handle;\n"), "{formatted}");
    assert!(validate(&formatted));
    for invalid in [
        "native { opaque Handle }",
        "native { opaque Handle {} }",
        "native { opaque Handle { value: Int } }",
        "native { opaque Handle { fn read(self: Handle) -> Int { return 0; } } }",
    ] {
        assert!(!validate(invalid), "accepted {invalid}");
    }
}

#[test]
fn removed_native_type_declaration_is_a_compile_error() {
    use rinzu_compiler::{DiagnosticCode, compile_source};
    let file = SourceFile::new(FileId(0), "old.rinzu", "native { type Handle; }");
    let diagnostics = compile_source(&file).unwrap_err();
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == DiagnosticCode::UnsupportedSafeFfi
            && diagnostic.message.contains("type T;")
    }));
}

#[test]
fn opaque_is_contextual_and_does_not_reserve_identifiers_or_type_aliases() {
    use rinzu_compiler::compile_source;
    let file = SourceFile::new(
        FileId(0),
        "contextual.rinzu",
        "type Value = Int; class Holder { const opaque: Value } \
         fn opaque(opaque: Int) -> Int { return opaque; } \
         fn main() { let holder = Holder(opaque: 3); let value = opaque(holder.opaque); }",
    );
    compile_source(&file).unwrap();
}

#[test]
fn source_names_and_records_are_validated() {
    assert!(validate(
        r#"native {
        opaque Handle;
        record Pair { x: CInt8, y: CDouble, valid: CBool }
        fn get(handle: Handle) -> Pair symbol "_get_pair";
    }"#
    ));
    for source in [
        r#"native { opaque T; fn T() -> Unit symbol "f"; }"#,
        r#"class T {} native { opaque T; }"#,
        r#"native { record T { x: Int, x: Int } }"#,
        r#"native { record T { x: String } }"#,
        r#"native { record T { x: Unit } }"#,
        r#"native { record T {} }"#,
        r#"native { userdata T; userdata T; }"#,
        r#"native { fn f(x: Int, x: Int) -> Unit symbol "f"; }"#,
        r#"native { async fn f(x: CInt32) -> Unit service "reactor.timer"; }"#,
        r#"native { async fn f(x: Int) -> Unit service "reactor.unknown"; }"#,
    ] {
        assert!(!validate(source), "accepted {source}");
    }
}

#[test]
fn symbols_are_ascii_c_identifiers() {
    for symbol in ["", "1bad", "with space", "mangled$", "é", "line\\nfeed"] {
        assert!(!validate(&format!(
            "native {{ fn f() -> Unit symbol \"{symbol}\"; }}"
        )));
    }
    assert!(validate(r#"native { fn f() -> Unit symbol "f_09"; }"#));
}

#[test]
fn signatures_preserve_nominal_shapes_and_one_argument_per_parameter() {
    use rinzu_compiler::{AbiScalar, SourceAbiType as A, source_native_signatures};
    let file = SourceFile::new(
        FileId(0),
        "native.rz",
        r#"
        let N = 4;
        type Context = {value:Int};
        native {
            opaque Handle;
            record Pair { x: CInt8, y: CDouble }
            fn invoke(h: Handle, p: Pair, out: Pair, ctx: Context,
                callback: fn(Pair, Context, Bytes(N)) -> String) -> Bytes(N) symbol "invoke";
        }
    "#,
    );
    let signatures = source_native_signatures(&parse_source(&file).unwrap()).unwrap();
    assert_eq!(signatures.len(), 1);
    let context = signatures[0].parameters[3].clone();
    assert!(matches!(&context, A::UserData(name) if name.starts_with("$rinzu_record_")));
    assert_eq!(
        signatures[0].parameters,
        vec![
            A::Opaque("Handle".into()),
            A::Record("Pair".into()),
            A::Record("Pair".into()),
            context.clone(),
            A::Callback {
                parameters: vec![A::Record("Pair".into()), context.clone(), A::Bytes(4)],
                result: Box::new(A::String)
            }
        ]
    );
    assert_eq!(signatures[0].result, A::Bytes(4));
    assert_eq!(
        A::Scalar(AbiScalar::I64),
        A::Scalar(AbiScalar::from_name("Int").unwrap())
    );
}

#[test]
fn foreign_callback_policy_uses_resolved_source_signatures() {
    use rinzu_compiler::source_native_signatures;
    let eligible = |signature: &str| {
        let source = format!(
            "type Context = {{value:Int}}; native {{ opaque Handle; record Pair {{ x: CInt }} fn invoke(callback: {signature}) -> Unit symbol \"invoke\"; }}"
        );
        let file = SourceFile::new(FileId(0), "foreign.rinzu", source);
        let signatures = source_native_signatures(&parse_source(&file).unwrap()).unwrap();
        signatures[0].parameters[0].foreign_callback_eligible()
    };
    for scalar in [
        "CInt8", "CInt16", "CInt32", "CInt64", "CUInt8", "CUInt16", "CUInt32", "CUInt64", "CFloat",
        "CDouble", "CBool", "Int", "Float", "Bool", "CInt", "CUInt", "CSize",
    ] {
        assert!(eligible(&format!("fn({scalar}) -> {scalar}")), "{scalar}");
    }
    for signature in [
        "fn() -> Unit",
        "fn(String, Bytes(0), Bytes(4)) -> String",
        "fn(String) -> Bytes(0)",
        "fn(Bytes(4)) -> Bytes(4)",
    ] {
        assert!(eligible(signature), "{signature}");
    }
    // These are legal same-thread callback signatures, but their nominal
    // wrappers/tokens may never be transported by the foreign entry.
    for signature in ["fn(Handle) -> Int", "fn(Pair) -> Int", "fn(Context) -> Int"] {
        assert!(!eligible(signature), "{signature}");
    }
}

#[test]
fn unsupported_abi_positions_are_rejected() {
    use rinzu_compiler::source_native_signatures;
    for declaration in [
        "fn f(x: Unit) -> Unit",
        "fn f(x: List<Int>) -> Unit",
        "supplant fn f() -> ByteArray",
        "fn f(x: fn() -> ByteArray) -> Unit",
        "fn f(x: fn(ByteArray) -> Int) -> Unit",
        "supplant fn f(x: ByteArray) -> Unit",
        "fn f(x: fn() -> Future<Int>) -> Unit",
        "fn f(x: supplant fn() -> Int) -> Unit",
        "fn f(x: fn(fn() -> Int) -> Int) -> Unit",
        "fn f(x: fn() -> Pair) -> Unit",
        "fn f(x: fn(NativeStorage<Pair>) -> Int) -> Unit",
        "fn f() -> NativeStorage<Pair>",
        "supplant fn f(x: NativeStorage<Pair>) -> Unit",
        "fn f() -> Context",
        "fn f(x: fn() -> Context) -> Unit",
        "fn f(x: NativeStorage<Context>) -> Unit",
        "fn f(N: Int, x: Bytes(N)) -> Unit",
        "fn f() -> fn() -> Int",
    ] {
        let text = format!(
            "let N = 4; class Context {{}} native {{ record Pair {{ x: Int }} {declaration} symbol \"f\"; }}"
        );
        let file = SourceFile::new(FileId(0), "native.rz", text);
        assert!(
            source_native_signatures(&parse_source(&file).unwrap()).is_err(),
            "accepted {declaration}"
        );
    }
    let file = SourceFile::new(FileId(0), "native.rz", "native { userdata Missing; }");
    assert!(parse_source(&file).is_err());
}

#[test]
fn dynamic_inputs_preserve_their_physical_contract() {
    use rinzu_compiler::{SourceAbiType, source_native_signatures};
    let file = SourceFile::new(
        FileId(0),
        "input.rinzu",
        r#"native { fn input(data:ByteArray,length:CSize)->Int symbol "input"; }"#,
    );
    let signatures = source_native_signatures(&parse_source(&file).unwrap()).unwrap();
    assert_eq!(signatures[0].parameters[0], SourceAbiType::ByteArray);
}

#[test]
fn dynamic_native_result_has_a_distinct_closed_abi_shape() {
    use rinzu_compiler::{SourceAbiType as A, source_native_signatures};
    let file = SourceFile::new(
        FileId(0),
        "bytes.rinzu",
        r#"native { fn copy(data:ByteArray,length:CSize)->ByteArray symbol "copy"; }"#,
    );
    let signatures = source_native_signatures(&parse_source(&file).unwrap()).unwrap();
    assert_eq!(signatures[0].parameters[0], A::ByteArray);
    assert_eq!(signatures[0].result, A::ByteArrayResult);
    assert_eq!(
        signatures[0].canonical_type(),
        "native(ByteArray,CUInt64)->NativeResult<ByteArray>"
    );
}
