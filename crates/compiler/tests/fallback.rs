use rinzu_compiler::{
    AbiScalar, FallbackConstant as F, FileId, SourceFile, compile_source, verify_module,
};

#[test]
fn fallback_constants_are_bound_to_actual_declarations() {
    let source = r#"
        fn integer() -> CUInt64 fallback 18446744073709551615 { return 0; }
        fn floating() -> CFloat fallback 1.000000059604644775390625000000000000000000000001 { return 0; }
        fn boolean() -> Bool fallback !(1 < 2) || true { return false; }
        fn text() -> String fallback "a\u{0}b" { return ""; }
        fn bytes() -> Bytes(4) fallback b"x" { return b""; }
        fn empty() fallback () {}
        fn main() { let alias = integer; }
    "#;
    let artifact = compile_source(&SourceFile::new(FileId(0), "fallback.rz", source)).unwrap();
    let lookup = |name| {
        artifact
            .ir
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap()
            .callback_fallback
            .clone()
            .unwrap()
    };
    assert_eq!(
        lookup("integer"),
        F::Integer {
            scalar: AbiScalar::U64,
            value: u64::MAX as i128
        }
    );
    assert_eq!(
        lookup("floating"),
        F::Float {
            scalar: AbiScalar::F32,
            bits: 0x3f80_0001
        }
    );
    assert_eq!(lookup("boolean"), F::Bool(true));
    assert_eq!(lookup("text"), F::String("a\0b".into()));
    assert_eq!(
        lookup("bytes"),
        F::Bytes {
            prefix: vec![b'x'],
            length: 4
        }
    );
    assert_eq!(lookup("empty"), F::Unit);
    assert!(artifact.checked.functions.iter().all(|f| !f.facts.may_fail));
    assert!(
        artifact
            .checked
            .reference_targets
            .values()
            .any(|targets| targets == &["integer"])
    );
}

#[test]
fn huge_bytes_fallback_is_a_compact_constant_plan() {
    let source = "let N: CSize = 18446744073709551615; fn callback() -> Bytes(N) fallback b\"\" { return b\"\"; } fn main() {}";
    let artifact = compile_source(&SourceFile::new(FileId(0), "fallback.rz", source)).unwrap();
    assert_eq!(
        artifact.ir.functions[0].callback_fallback,
        Some(F::Bytes {
            prefix: vec![],
            length: u64::MAX
        })
    );
}

#[test]
fn runtime_values_traps_async_and_unsupported_abi_results_are_rejected() {
    for declaration in [
        "fn callback(x: Int) -> Int fallback x { return x; }",
        "fn callback() -> Int fallback helper() { return 0; }",
        "fn callback() -> Int fallback 1 / 0 { return 0; }",
        "fn callback() -> CInt8 fallback 127 + 1 { return 0; }",
        "fn callback() -> Bool fallback 1 { return false; }",
        "fn callback() -> Bytes(1) fallback b\"xx\" { return b\"\"; }",
        "async fn callback() -> Int fallback 0 { return 0; }",
        "fn callback() -> Context fallback () { return Context(value: 0); }",
        "fn callback() -> Int fallback N { return 0; }",
    ] {
        let source = format!(
            "let N = 1; class Context {{ const value: Int }} fn helper() -> Int {{ return 1; }} {declaration} fn main() {{}}"
        );
        assert!(
            compile_source(&SourceFile::new(FileId(0), "fallback.rz", source)).is_err(),
            "accepted {declaration}"
        );
    }
}
#[test]
fn verifier_rejects_a_fallback_with_wrong_result_type() {
    let source = SourceFile::new(
        FileId(0),
        "fallback.rz",
        "fn callback() -> Int fallback 0 { return 1; } fn main() {}",
    );
    let mut artifact = compile_source(&source).unwrap();
    artifact.ir.functions[0].callback_fallback = Some(F::Bool(false));
    assert!(verify_module(&artifact.ir).is_err());
}
