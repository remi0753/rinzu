use rinzu_compiler::{
    AbiScalar, ConstantValue, FileId, OperationKind, SourceFile, Type, compile_source,
};
fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(1), "scalar.rinzu", text))
}

#[test]
fn scalar_aliases_contextual_literals_and_arithmetic_reach_typed_ir() {
    let artifact = compile(
        r#"
let upper: CUInt64 = 18_446_744_073_709_551_615;
let size: CInt8 = next + 1;
let next: CInt8 = 4;
fn small(input: CInt8) -> CInt8 { return input + (1 + 2); }
fn main() {
    let negative: CInt8 = -128;
    let wide: CSize = upper;
    let narrow = small(10);
    let comparison = 1 < narrow;
    let rounded: CFloat = 1.000000059604644775390625000000000000000000001;
    let normal: CDouble = 1;
    let enabled: CBool = true;
    let same = CInt64(1);
    try { let converted = CInt8(200); } catch errors {}
}
"#,
    )
    .unwrap();
    assert_eq!(artifact.checked.constants["upper"].value, u64::MAX as i128);
    assert_eq!(artifact.checked.constants["size"].scalar, AbiScalar::I8);
    let small = artifact
        .checked
        .functions
        .iter()
        .find(|function| function.declaration.name == "small")
        .unwrap();
    assert_eq!(small.parameter_types, [Type::Scalar(AbiScalar::I8)]);
    assert_eq!(small.result_type, Type::Scalar(AbiScalar::I8));
    let operations = artifact
        .ir
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
        .collect::<Vec<_>>();
    assert!(operations.iter().any(|operation| {
        matches!(
            operation.kind,
            OperationKind::Constant(ConstantValue::Int(-1))
        ) && operation
            .result
            .as_ref()
            .is_some_and(|result| result.ty == Type::Scalar(AbiScalar::U64))
    }));
    assert!(operations.iter().any(|operation| {
        matches!(
            operation.kind,
            OperationKind::Constant(ConstantValue::Int(-128))
        ) && operation
            .result
            .as_ref()
            .is_some_and(|result| result.ty == Type::Scalar(AbiScalar::I8))
    }));
    assert!(operations.iter().any(|operation| matches!(operation.kind, OperationKind::Constant(ConstantValue::Float(value)) if value == f64::from(f32::from_bits(0x3f800001)))
        && operation.result.as_ref().is_some_and(|result| result.ty == Type::Scalar(AbiScalar::F32))));
    assert_eq!(
        operations
            .iter()
            .filter(|operation| matches!(operation.kind, OperationKind::NumericConvert { .. }))
            .count(),
        1
    );
}

#[test]
fn module_constants_lower_to_values_and_lexical_shadowing_stays_local() {
    let artifact = compile("let count = 4; fn main() { let fromConstant = count; if true { let count = 2; let fromLocal = count; } let after = count; }").unwrap();
    let loads = artifact.ir.functions[0].blocks.iter().flat_map(|block| &block.operations)
        .filter(|operation| matches!(&operation.kind, OperationKind::LoadLocal { name } if name == "count")).count();
    assert_eq!(loads, 1);
    assert_eq!(artifact.checked.constant_references.len(), 2);
    assert!(
        !artifact
            .ir
            .functions
            .iter()
            .any(|function| function.name == "count")
    );
}

#[test]
fn rejects_out_of_range_literals_implicit_conversion_and_invalid_numeric_operations() {
    for text in [
        "fn main() { let bad: CInt8 = 128; }",
        "fn main() { let bad: CInt8 = -129; }",
        "fn main() { let bad: CUInt8 = -1; }",
        "fn main() { let bad = 18446744073709551615; }",
        "fn main() { let small: CInt8 = 1; let bad: Int = small + 1; }",
        "fn main() { let small: CInt8 = 1; let wide: Int = 1; let bad = small + wide; }",
        "fn main() { let bad: CInt8 = 1.0; }",
        "fn main() { let bad = CInt8(true); }",
        "fn main() { let bad = CInt8(); }",
        "fn main() { let bad = CInt8(1,2); }",
        "fn main() { let bad = CFloat(1) % CFloat(2); }",
        "fn main() { let bad = CUInt64(18446744073709551615); }",
    ] {
        assert!(compile(text).is_err(), "{text}");
    }
    // Conversion remains an executable operation; Main must handle its Error.
    assert!(compile("fn main() { try { let value = CInt8(200); } catch errors {} }").is_ok());
}

#[test]
fn typed_metadata_keeps_file_identity_when_offsets_coincide() {
    let first = rinzu_compiler::parse_source(&SourceFile::new(
        FileId(1),
        "one.rinzu",
        "let one = 1; fn first()->Int { return one; }",
    ))
    .unwrap();
    let second = rinzu_compiler::parse_source(&SourceFile::new(
        FileId(2),
        "two.rinzu",
        "let two = 2; fn other()->Int { return two; }",
    ))
    .unwrap();
    let mut merged = first;
    merged.declarations.extend(second.declarations);
    merged.declarations.extend(
        rinzu_compiler::parse_source(&SourceFile::new(FileId(3), "main.rinzu", "fn main() {}"))
            .unwrap()
            .declarations,
    );
    let checked = rinzu_compiler::check_module(&merged).unwrap();
    assert_eq!(checked.constant_references.len(), 2);
    let values = checked
        .constant_references
        .iter()
        .map(|(range, value)| (range.file_id, value.value))
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(values[&FileId(1)], 1);
    assert_eq!(values[&FileId(2)], 2);
}

#[test]
fn integer_literals_round_directly_into_float_without_a_u64_intermediate() {
    let artifact =
        compile("fn main() { let value: CFloat = 340282346638528859811704183484516925440; }")
            .unwrap();
    assert!(artifact.ir.functions[0].blocks.iter().flat_map(|block| &block.operations)
        .any(|operation| matches!(operation.kind, OperationKind::Constant(ConstantValue::Float(value)) if value == f64::from(f32::MAX))));
    assert!(compile("fn main() { let value: CUInt64 = 18446744073709551616; }").is_err());
}

#[test]
fn numeric_separators_are_allowed_only_between_digits() {
    for value in ["1_", "1__2", "1_.0", "1.0_", "1e_2"] {
        assert!(compile(&format!("fn main() {{ let value = {value}; }}")).is_err());
    }
    compile("fn main() { let value = 1_000; let exponent = 1.0e1_0; }").unwrap();
}

#[test]
fn scalar_remainder_keeps_its_operand_type() {
    let file = SourceFile::new(
        FileId(0),
        "remainder.rz",
        "fn remainder(x: CUInt8) -> CUInt8 { return x % 2; } fn main() {}",
    );
    let artifact = compile_source(&file).unwrap();
    let function = artifact
        .checked
        .functions
        .iter()
        .find(|f| f.declaration.name == "remainder")
        .unwrap();
    assert_eq!(function.result_type, Type::Scalar(AbiScalar::U8));
}

#[test]
fn unsigned_unary_minus_is_rejected_even_for_zero() {
    for source in [
        "fn main() { let x: CUInt8 = -0; }",
        "fn main() { let x: CUInt8 = 0; let y = -x; }",
        "let N: CUInt8 = -0; fn main() {}",
        "fn callback() -> CUInt8 fallback -0 { return 0; } fn main() {}",
    ] {
        assert!(compile_source(&SourceFile::new(FileId(0), "unsigned.rz", source)).is_err());
    }
}
