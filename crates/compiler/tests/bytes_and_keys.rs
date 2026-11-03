use rinzu_compiler::{FileId, OperationKind, SourceFile, Type, compile_source, verify_module};
fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(1), "bytes.rinzu", text))
}

#[test]
fn fixed_lengths_lexical_constants_and_padding_are_explicit_in_ir() {
    let artifact = compile(
        r#"
let blockSize = 4;
fn widen(input: Bytes(2)) -> Bytes(blockSize) { return input; }
fn main() {
    let localSize = blockSize + 4;
    let small: Bytes(blockSize) = b"AB";
    let large: Bytes(localSize) = small;
    let empty: Bytes(0) = b"";
    let same: Bytes(2+2) = small;
    var variable: Bytes(4) = b"A";
    variable = b"B";
    let list: List<Bytes(4)> = [b"X", b"Y"];
}
"#,
    )
    .unwrap();
    let widen = artifact
        .checked
        .functions
        .iter()
        .find(|function| function.declaration.name == "widen")
        .unwrap();
    assert_eq!(widen.result_type, Type::FixedBytes(4));
    assert_eq!(widen.parameter_types, [Type::FixedBytes(2)]);
    let pads = artifact
        .ir
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
        .filter_map(|operation| match operation.kind {
            OperationKind::BytesPad { from, to, .. } => Some((from, to)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(pads.contains(&(2, 4)) && pads.contains(&(4, 8)) && pads.contains(&(1, 4)));
    assert!(pads.iter().all(|(from, to)| from < to));
    assert!(
        artifact
            .checked
            .expression_types
            .values()
            .any(|ty| *ty == Type::FixedBytes(0))
    );
}

#[test]
fn largest_csize_is_metadata_without_compiler_materializing_a_buffer() {
    let artifact = compile(
        "let size: CSize = 18446744073709551615; fn main() { let value: Bytes(size) = b\"\"; }",
    )
    .unwrap();
    assert!(
        artifact.ir.functions[0]
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .any(|operation| matches!(
                operation.kind,
                OperationKind::BytesPad {
                    from: 0,
                    to: u64::MAX,
                    ..
                }
            ))
    );
}

#[test]
fn rejects_bare_lengths_runtime_dependencies_shadow_fallback_and_truncation() {
    for text in [
        "fn main() { let bad: Bytes = b\"A\"; }",
        "fn main() { let bad: Bytes(-1) = b\"\"; }",
        "fn main() { let bad: Bytes(1.0) = b\"\"; }",
        "fn main() { let bad: Bytes(true) = b\"\"; }",
        "fn main() { let bad: Bytes(1/0) = b\"\"; }",
        "fn main() { var size = 4; let bad: Bytes(size) = b\"\"; }",
        "fn number()->Int { return 4; } fn main() { let size = number(); let bad: Bytes(size) = b\"\"; }",
        "fn main() { let size = Int(4); let bad: Bytes(size) = b\"\"; }",
        "fn main() { let bad: Bytes(2) = b\"ABC\"; }",
        "fn main() { let value = b\"ABC\"; let bad: Bytes(2) = value; }",
        "let size = 4; fn body(size: Int) { let bad: Bytes(size) = b\"\"; } fn main() {}",
        "let size = 4; fn body(size: Int, bytes: Bytes(size)) {} fn main() {}",
        "fn main() { if true { let size = 4; } let bad: Bytes(size) = b\"\"; }",
    ] {
        assert!(compile(text).is_err(), "{text}");
    }
}

#[test]
fn revised_key_set_rejects_all_forbidden_types_even_on_empty_maps() {
    compile("enum Key { item(value: CUInt8), label(text: String), empty } fn main() { let integers: Map<CSize, Bool> = {}; let strings: Map<String, Bytes(2)> = {}; let keys: Map<Key, Int> = {}; }").unwrap();
    for key in [
        "Float",
        "CFloat",
        "Unit",
        "BadRecord",
        "BadEnum",
        "List<Int>",
        "Map<Int,Int>",
        "Error",
        "Future<Unit>",
    ] {
        let text = format!(
            "class BadRecord {{ const number: Float }} enum BadEnum {{ empty, bad(value: Float) }} fn main() {{ let values: Map<{key}, Int> = {{}}; }}"
        );
        assert!(compile(&text).is_err(), "{key}");
    }
    compile("fn useMap(value: Map<Bool, Int>) {} fn main() {}").unwrap();
}

#[test]
fn verifier_rejects_padding_that_misstates_its_source_length() {
    let mut artifact = compile("fn main() { let value: Bytes(4) = b\"AB\"; }").unwrap();
    for operation in artifact.ir.functions[0]
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.operations)
    {
        if let OperationKind::BytesPad { from, .. } = &mut operation.kind {
            *from = 3;
        }
    }
    assert!(verify_module(&artifact.ir).is_err());
}
