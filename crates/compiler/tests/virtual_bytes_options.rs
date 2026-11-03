use rinzu_compiler::{
    FileId, IrFunction, OperationKind, SourceFile, Type, compile_source, virtual_bytes_options,
};

fn functions() -> Vec<IrFunction> {
    compile_source(&SourceFile::new(
        FileId(92),
        "virtual-bytes-options.rinzu",
        r#"
fn leaf(data: ByteArray, start: Int, end: Int) -> ByteArray {
    switch bytesSlice(data, start, end) {
        case Option.Some(piece): { return piece; }
        case Option.None: { return byteArray(b"invalid"); }
    }
}
fn escaping(data: ByteArray) -> Option<ByteArray> { return bytesSlice(data, 0, 1); }
fn alias(data: ByteArray) -> ByteArray {
    let candidate = bytesSlice(data, 0, 1);
    switch candidate {
        case Option.Some(piece): { return piece; }
        case Option.None: { return byteArray(b"invalid"); }
    }
}
fn identity(data: ByteArray) -> ByteArray { return data; }
fn calling(data: ByteArray) -> ByteArray {
    switch bytesSlice(data, 0, 1) {
        case Option.Some(piece): { return identity(piece); }
        case Option.None: { return byteArray(b"invalid"); }
    }
}
async fn asynchronous(data: ByteArray) -> ByteArray {
    switch bytesSlice(data, 0, 1) {
        case Option.Some(piece): { return piece; }
        case Option.None: { return byteArray(b"invalid"); }
    }
}
fn scalar(data: ByteArray) -> Int {
    switch bytesGet(data, 0) {
        case Option.Some(value): { return Int(value); }
        case Option.None: { return -1; }
    }
}
fn main() {}
"#,
    ))
    .unwrap()
    .ir
    .functions
}

#[test]
fn only_the_closed_immediate_leaf_switch_has_a_virtual_reference_option() {
    for function in functions() {
        let plan = virtual_bytes_options(&function);
        if function.name == "leaf" {
            assert!(plan.len() >= 2);
            let payload = function
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .find(|op| matches!(op.kind, OperationKind::EnumPayload { .. }))
                .unwrap();
            let OperationKind::EnumPayload { operand, .. } = &payload.kind else {
                unreachable!()
            };
            assert!(plan.contains(operand));
            assert_eq!(payload.result.as_ref().unwrap().ty, Type::Bytes);
        } else {
            assert!(plan.is_empty(), "{}: {plan:?}", function.name);
        }
    }
}

#[test]
fn a_wrong_case_or_reassigned_switch_temporary_cannot_use_the_representation() {
    let leaf = functions().into_iter().find(|f| f.name == "leaf").unwrap();
    let mut wrong_case = leaf.clone();
    for op in wrong_case.blocks.iter_mut().flat_map(|b| &mut b.operations) {
        if let OperationKind::EnumPayload { variant, .. } = &mut op.kind {
            *variant = 0;
        }
    }
    assert!(virtual_bytes_options(&wrong_case).is_empty());
    let mut reassigned = leaf;
    for block in &mut reassigned.blocks {
        if let Some(store) = block.operations.iter().find(|op| {
            matches!(&op.kind, OperationKind::StoreLocal { name, .. } if name.starts_with("$switch"))
        }).cloned() {
            block.operations.push(store);
            break;
        }
    }
    assert!(virtual_bytes_options(&reassigned).is_empty());
}
