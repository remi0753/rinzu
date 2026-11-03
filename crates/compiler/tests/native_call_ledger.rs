use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source, verify_module};

fn artifact() -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(
        FileId(0),
        "native-ledger.rz",
        r#"
        native { fn call(cb: fn(Int) -> Int, text: String) -> Int symbol "call"; }
        fn callback(x: Int) -> Int fallback -1 { return x; }
        fn main() { let alias = callback; try { let result = call(alias, "text"); } catch error {} }
    "#,
    ))
    .unwrap()
}
#[test]
fn arguments_callbacks_and_result_follow_one_balanced_call_ledger() {
    let artifact = artifact();
    let block = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .find(|b| {
            b.operations
                .iter()
                .any(|op| matches!(op.kind, OperationKind::PrepareNativeCall { .. }))
        })
        .unwrap();
    let index = block
        .operations
        .iter()
        .position(|op| matches!(op.kind, OperationKind::PrepareNativeCall { .. }))
        .unwrap();
    let OperationKind::PrepareNativeCall {
        call,
        arguments,
        retained,
        callbacks,
        error: prepare_error,
        ..
    } = &block.operations[index].kind
    else {
        unreachable!()
    };
    assert_eq!(arguments.len(), 2);
    assert_eq!(retained, arguments);
    assert_eq!(callbacks.len(), 1);
    assert_eq!(callbacks[0].index, 0);
    assert_eq!(callbacks[0].targets, ["callback"]);
    assert!(prepare_error.release_native_calls.is_empty());
    assert!(
        block.operations[index]
            .unwind
            .as_ref()
            .unwrap()
            .release_native_calls
            .is_empty()
    );
    assert_ne!(
        prepare_error.target,
        artifact
            .ir
            .functions
            .iter()
            .find(|f| f.name == "main")
            .unwrap()
            .failure
    );
    assert!(
        matches!(block.operations[index + 1].kind, OperationKind::SourceNativeCall { call: Some(id), .. } if id == *call)
    );
    assert!(block.operations[index + 1].result.is_none());
    assert!(
        matches!(block.operations[index + 2].kind, OperationKind::CloseNativeCall { call: id } if id == *call)
    );
    assert!(
        matches!(block.operations[index + 3].kind, OperationKind::SettleNativeAdmissions { call: id, .. } if id == *call)
    );
    assert!(
        matches!(block.operations[index + 4].kind, OperationKind::ReadNativeResult { call: id, .. } if id == *call)
    );
    assert!(
        matches!(block.operations[index + 5].kind, OperationKind::ReleaseNativeCall { call: id } if id == *call)
    );
    let OperationKind::ReadNativeResult { error, .. } = &block.operations[index + 4].kind else {
        unreachable!()
    };
    assert_eq!(error.release_native_calls, [*call]);
    assert_eq!(
        block.operations[index + 4]
            .unwind
            .as_ref()
            .unwrap()
            .release_native_calls,
        [*call]
    );
}
#[test]
fn verifier_rejects_missing_phases_roots_target_identity_and_double_release() {
    let artifact = artifact();
    for mutation in 0..10 {
        let mut ir = artifact.ir.clone();
        let block = ir
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .find(|b| {
                b.operations
                    .iter()
                    .any(|op| matches!(op.kind, OperationKind::PrepareNativeCall { .. }))
            })
            .unwrap();
        let index = block
            .operations
            .iter()
            .position(|op| matches!(op.kind, OperationKind::PrepareNativeCall { .. }))
            .unwrap();
        match mutation {
            0 => {
                block.operations.remove(index + 2);
            }
            1 => {
                block.operations.remove(index + 3);
            }
            2 => {
                block.operations.remove(index + 5);
            }
            3 => {
                let release = block.operations[index + 5].clone();
                block.operations.push(release);
            }
            4 => {
                if let OperationKind::PrepareNativeCall { retained, .. } =
                    &mut block.operations[index].kind
                {
                    retained.clear();
                }
            }
            5 => {
                if let OperationKind::PrepareNativeCall { callbacks, .. } =
                    &mut block.operations[index].kind
                {
                    callbacks[0].targets = vec!["missing".into()];
                }
            }
            6 => {
                if let OperationKind::ReadNativeResult { error, .. } =
                    &mut block.operations[index + 4].kind
                {
                    error.release_native_calls.clear();
                }
            }
            7 => {
                if let OperationKind::PrepareNativeCall { callbacks, .. } =
                    &mut block.operations[index].kind
                {
                    callbacks[0].targets.push("callback".into());
                }
            }
            8 => {
                if let OperationKind::PrepareNativeCall { call, error, .. } =
                    &mut block.operations[index].kind
                {
                    error.release_native_calls.push(*call);
                }
            }
            9 => {
                block.operations[index]
                    .unwind
                    .as_mut()
                    .unwrap()
                    .release_native_calls
                    .push(rinzu_compiler::NativeCallId(0));
            }
            _ => unreachable!(),
        }
        assert!(verify_module(&ir).is_err(), "accepted mutation {mutation}");
    }
}

#[test]
fn core_callback_plans_close_over_exact_lengths_and_reject_corrupted_targets() {
    let artifact = compile_source(&SourceFile::new(
        FileId(0),
        "core-callback.rinzu",
        r#"
        let width: CSize = 2 + 2;
        native {
            fn zero(cb: fn(Bytes(0)) -> CSize) -> CSize symbol "zero";
            fn four(cb: fn(Bytes(width)) -> CSize) -> CSize symbol "four";
            fn text(cb: fn(String) -> CSize) -> CSize symbol "text";
        }
        fn main() {
            let alias: fn(Bytes(width)) -> CUInt64 = byteLength;
            let z = zero(byteLength);
            let n = four(alias);
            let t = text(measure);
        }
        fn measure(value:String)->CSize fallback 0 {return utf8ByteLength(value);}
    "#,
    ))
    .unwrap();
    let targets = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter_map(|op| match &op.kind {
            OperationKind::PrepareNativeCall { callbacks, .. } => {
                Some(callbacks[0].targets[0].clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            "rinzu.core::byteLength:0",
            "rinzu.core::byteLength:4",
            "measure"
        ]
    );
    for target in [
        "rinzu.core::byteLength:04",
        "rinzu.core::byteLength:8",
        "rinzu.core::utf8ByteLength",
    ] {
        let mut ir = artifact.ir.clone();
        let operation = ir.functions.iter_mut().flat_map(|f| &mut f.blocks).flat_map(|b| &mut b.operations)
            .find(|op| matches!(&op.kind, OperationKind::PrepareNativeCall { signature, .. } if signature.name == "four")).unwrap();
        if let OperationKind::PrepareNativeCall { callbacks, .. } = &mut operation.kind {
            callbacks[0].targets[0] = target.into();
        }
        assert!(verify_module(&ir).is_err(), "accepted {target}");
    }
}
