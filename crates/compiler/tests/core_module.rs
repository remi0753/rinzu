use rinzu_compiler::{
    AbiScalar, CoreLengthKind, FileId, OperationKind, SourceFile, Type, compile_source,
    verify_module,
};

fn compile(body: &str) -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(FileId(1), "core.rinzu", body)).unwrap()
}

#[test]
fn length_functions_have_csize_results_and_distinct_typed_operations() {
    let artifact = compile(
        "import rinzu.core; fn main() { let text: CSize = utf8ByteLength(\"あ\\u{0}\"); let bytes: CSize = rinzu.core.byteLength(b\"x\"); }",
    );
    let operations: Vec<_> = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter(|op| matches!(op.kind, OperationKind::CoreLength { .. }))
        .collect();
    assert_eq!(operations.len(), 2);
    assert!(matches!(
        operations[0].kind,
        OperationKind::CoreLength {
            kind: CoreLengthKind::Utf8,
            ..
        }
    ));
    assert!(matches!(
        operations[1].kind,
        OperationKind::CoreLength {
            kind: CoreLengthKind::Bytes,
            ..
        }
    ));
    assert!(
        operations
            .iter()
            .all(|op| op.result.as_ref().unwrap().ty == Type::Scalar(AbiScalar::U64))
    );
    let mut bad = artifact.ir.clone();
    let operation = bad
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
        .find(|op| matches!(op.kind, OperationKind::CoreLength { .. }))
        .unwrap();
    operation.result.as_mut().unwrap().ty = Type::Int;
    assert!(verify_module(&bad).is_err());
}

#[test]
fn prelude_and_imported_reference_share_identity_and_worker_effects() {
    let artifact = compile(
        "import rinzu.core; fn helper() -> CSize { let first = utf8ByteLength; let second = rinzu.core.utf8ByteLength; return first(\"x\") + second(\"y\"); } fn main() { let value = helper(); }",
    );
    let targets: Vec<_> = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter_map(|op| match &op.kind {
            OperationKind::FunctionReference { target } => Some(target.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        targets,
        ["rinzu.core::utf8ByteLength", "rinzu.core::utf8ByteLength"]
    );
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .find(|f| f.declaration.name == "helper")
            .unwrap()
            .facts
            .worker_safe
    );
}

#[test]
fn invalid_arguments_missing_import_and_application_storage_are_rejected() {
    for body in [
        "fn main() { let value = utf8ByteLength(1); }",
        "fn main() { let value = byteLength(\"x\"); }",
        "fn main() { let value = utf8ByteLength(); }",
        "fn main() { let value = rinzu.core.utf8ByteLength(\"x\"); }",
        "import rinzu.core; fn main() { let value = rinzu.core.nativeCopy(1); }",
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(1), "core.rinzu", body)).is_err(),
            "accepted {body}"
        );
    }
}

#[test]
fn byte_length_references_close_over_the_expected_bytes_length() {
    let artifact = compile(
        "fn main() { let size: fn(Bytes(4)) -> CSize = byteLength; let value = size(b\"x\"); }",
    );
    assert!(artifact.ir.functions.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.operations).any(|op| matches!(&op.kind, OperationKind::FunctionReference { target } if target == "rinzu.core::byteLength:4")));
}

#[test]
fn length_reference_identity_is_canonical_and_incompatible_types_do_not_coerce() {
    for body in [
        "fn main() { let measure = byteLength; let result=measure(b\"x\"); }",
        "fn main() { let measure: fn(Bytes(4))->Int = byteLength; }",
        "fn main() { let measure: fn(Bytes(4),Int)->CSize = byteLength; }",
        "fn main() { let first: fn(Bytes(4))->CSize=byteLength; let second: fn(Bytes(8))->CSize=first; }",
        "fn main() { let measure: async fn(Bytes(4))->CSize=byteLength; }",
        "fn main() { let measure: supplant fn(Bytes(4))->CSize=byteLength; }",
        "fn worker(measure: fn(Bytes(4))->CSize) -> CSize { return measure(b\"x\"); } fn main() { let measure: fn(Bytes(4))->CSize=byteLength; let task=spawn worker(measure); }",
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(1), "bad-reference.rinzu", body)).is_err(),
            "accepted {body}"
        );
    }
    let mut artifact = compile(
        "fn main() { let measure: fn(Bytes(4))->CSize=byteLength; let size=measure(b\"x\"); }",
    );
    for operation in artifact
        .ir
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
    {
        if let OperationKind::FunctionReference { target } = &mut operation.kind {
            *target = "rinzu.core::byteLength:04".to_owned();
        }
    }
    assert!(verify_module(&artifact.ir).is_err());
    for targets in [
        vec!["rinzu.core::byteLength:8".to_owned()],
        vec!["rinzu.core::byteLength:4".to_owned(); 2],
    ] {
        let mut artifact = compile(
            "fn main() { let measure: fn(Bytes(4))->CSize=byteLength; let size=measure(b\"x\"); }",
        );
        for operation in artifact
            .ir
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.operations)
        {
            if let OperationKind::IndirectCall {
                targets: possible, ..
            } = &mut operation.kind
            {
                *possible = targets.clone();
            }
        }
        assert!(verify_module(&artifact.ir).is_err());
    }
    compile(
        "fn apply(target: fn(Int)->Int,value: Int)->Int { return target(value); } fn main() {}",
    );
}
