use rinzu_compiler::{FileId, OperationKind, SourceFile, Type, compile_source, verify_module};

fn compile(source: &str) -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(FileId(0), "native.rz", source)).unwrap()
}

#[test]
fn ordinary_source_call_preserves_physical_signature_and_worker_safety() {
    let artifact = compile(
        r#"
        native { fn add(x: Int, y: Int) -> Int symbol "external_add"; }
        fn work(x: Int) -> Int { return add(x, 2); }
        fn main() { let result = work(40); }
    "#,
    );
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .all(|f| f.facts.worker_safe)
    );
    let operation = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .find(|op| matches!(op.kind, OperationKind::SourceNativeCall { .. }))
        .unwrap();
    let OperationKind::SourceNativeCall {
        signature,
        arguments,
        ..
    } = &operation.kind
    else {
        unreachable!()
    };
    assert_eq!(signature.name, "add");
    assert_eq!(arguments.len(), 2);
    assert!(operation.result.is_none());
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(
                |op| matches!(op.kind, OperationKind::ReadNativeResult { .. })
                    && op
                        .result
                        .as_ref()
                        .is_some_and(|value| value.ty == Type::Int)
            )
    );
    assert!(
        !artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|op| matches!(op.kind, OperationKind::NativeCall { .. }))
    );
}

#[test]
fn synchronous_callback_and_async_service_are_typed() {
    let artifact = compile(
        r#"
        import rinzu.timer;
async fn delay(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }
native {
            fn invoke(callback: fn(Int) -> Int) -> Int symbol "invoke";

        }
        fn callback(x: Int) -> Int { return x + 1; }
        async fn main() { try { let result = invoke(callback); await delay(1); } catch errors {} }
    "#,
    );
    assert_eq!(artifact.checked.source_native_calls.len(), 2);
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|op| matches!(op.kind, OperationKind::FunctionReference { .. }))
    );
}

#[test]
fn call_types_and_verifier_enforce_signature() {
    for source in [
        r#"native { fn f(x: Int) -> Int symbol "f"; } fn main() { let result = f(true); }"#,
        r#"native { fn f(x: Int) -> Int symbol "f"; } fn main() { let result = f(); }"#,
        r#"native { fn f(x: fn(Int) -> Int) -> Int symbol "f"; } fn cb(x: Bool) -> Int { return 1; } fn main() { let result = f(cb); }"#,
    ] {
        assert!(compile_source(&SourceFile::new(FileId(0), "native.rz", source)).is_err());
    }
    let mut artifact =
        compile(r#"native { fn f(x: Int) -> Int symbol "f"; } fn main() { let result = f(1); }"#);
    let operation = artifact
        .ir
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
        .find(|op| matches!(op.kind, OperationKind::ReadNativeResult { .. }))
        .unwrap();
    operation.result.as_mut().unwrap().ty = Type::Bool;
    assert!(verify_module(&artifact.ir).is_err());
}
