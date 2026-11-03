use rinzu_compiler::{AbiScalar, FileId, SourceFile, evaluate_module_constants, parse_source};
fn evaluate(
    text: &str,
) -> Result<
    std::collections::BTreeMap<String, rinzu_compiler::IntegerConstant>,
    Vec<rinzu_compiler::Diagnostic>,
> {
    evaluate_module_constants(
        &parse_source(&SourceFile::new(FileId(1), "constants.rinzu", text)).unwrap(),
    )
}

#[test]
fn canonical_aliases_and_full_integer_ranges() {
    for (a, b) in [
        ("Int", "CInt64"),
        ("Float", "CDouble"),
        ("Bool", "CBool"),
        ("CInt", "CInt32"),
        ("CUInt", "CUInt32"),
        ("CSize", "CUInt64"),
    ] {
        assert_eq!(AbiScalar::from_name(a), AbiScalar::from_name(b));
    }
    for (name, min, max) in [
        ("CInt8", -128, 127),
        ("CInt16", -32768, 32767),
        ("CInt32", i32::MIN as i128, i32::MAX as i128),
        ("Int", i64::MIN as i128, i64::MAX as i128),
        ("CUInt8", 0, 255),
        ("CUInt16", 0, 65535),
        ("CUInt32", 0, u32::MAX as i128),
        ("CUInt64", 0, u64::MAX as i128),
    ] {
        let values = evaluate(&format!(
            "let low: {name} = {min}; let high: {name} = {max};"
        ))
        .unwrap();
        assert_eq!(values["low"].value, min);
        assert_eq!(values["high"].value, max);
        assert!(evaluate(&format!("let low: {name} = {};", min - 1)).is_err());
        // Values beyond the largest lexer magnitude are still rejected at the source boundary.
        if max < u64::MAX as i128 {
            assert!(evaluate(&format!("let high: {name} = {};", max + 1)).is_err());
        }
    }
}

#[test]
fn dependency_order_contextual_arithmetic_and_no_runtime_calls() {
    let values = evaluate("let size = next + 1; let next = base * 2; let base: CInt8 = 4; let wide: CUInt64 = 18_446_744_073_709_551_615;").unwrap();
    assert_eq!(values["size"].value, 9);
    assert_eq!(values["size"].scalar, AbiScalar::I8);
    assert_eq!(values["wide"].value, u64::MAX as i128);
    let values = evaluate(
        "let signed: CInt8 = -128; let unsigned: CUInt8 = 255; let grouped: CInt16 = (1+2)*3;",
    )
    .unwrap();
    assert_eq!(values["signed"].value, -128);
    assert_eq!(values["grouped"].value, 9);
}

#[test]
fn rejects_cycles_traps_mixed_types_and_nonconstant_initializers() {
    for text in [
        "let a = b; let b = a;",
        "let a = 1; let a = 2;",
        "let value: CInt8 = -128 / -1;",
        "let value: CInt8 = -128 % -1;",
        "let a = a;",
        "let value: CInt8 = 127+1;",
        "let value: CUInt8 = 0-1;",
        "let value = 1/0;",
        "let value = 1%0;",
        "let value = missing;",
        "let value = read();",
        "let value = Int(1);",
        "let value = await read();",
        "let value = spawn run();",
        "let value = 0.5;",
        "let value: CBool = 1;",
        "let value: Bytes(4) = 1;",
        "let small: CInt8 = 1; let wide: Int = small;",
        "let narrow: CInt8 = 100 * 2 - 100;",
    ] {
        assert!(
            rinzu_compiler::compile_source(&SourceFile::new(
                FileId(1),
                "constants.rinzu",
                format!("{text} fn main() {{}}")
            ))
            .is_err(),
            "{text}"
        );
    }
}
