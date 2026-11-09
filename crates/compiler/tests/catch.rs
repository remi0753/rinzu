use rinzu_compiler::{FileId, OperationKind, SourceFile, Type, compile_source, verify_module};

fn compile(source: &str) -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(FileId(0), "catch.rz", source)).unwrap()
}
#[test]
fn error_message_and_catch_scopes_are_typed() {
    let artifact = compile(
        r#"
        class Report { const error: Error }
        fn message(error: Error) -> String { return error.message; }
        fn main() {
            try { let x = CInt8(200); }
            catch [error: Error, ...errorRest] { let text = message(error); let report = Report(error: error); }
            finally { let completed = true; }
        }
    "#,
    );
    assert!(
        artifact
            .checked
            .expression_types
            .values()
            .any(|ty| *ty == Type::Error)
    );
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|op| matches!(
                op.kind,
                OperationKind::CatchErrors {
                    observes_failure: true,
                    ..
                }
            ))
    );
    for body in [
        "try {} catch e { e = e; }",
        "try { let inner = 1; } catch e { let x = inner; }",
        "try {} catch e {} finally { let x = e; }",
        "try {} catch e {} let x = e;",
        "let e = 1; try {} catch e {}",
        "try {} catch e { let x = e == e; }",
        "try {} catch e { let x = {e: 1}; }",
        "try {} catch e { let x = e.code; }",
        "let x = Error();",
    ] {
        let source = format!("fn main() {{ {body} }}");
        assert!(
            compile_source(&SourceFile::new(FileId(0), "catch.rz", source)).is_err(),
            "accepted {body}"
        );
    }
}
#[test]
fn operation_error_edges_skip_own_finally_while_traps_bypass_catch() {
    let artifact = compile(
        r#"
        fn main() {
            try {
                try { let converted = CInt8(200); let trap = 1 / 0; }
                finally { let inner_cleanup = true; }
            } catch [error: Error, ...errorRest] { let text = error.message; }
            finally { let outer_cleanup = true; }
        }
    "#,
    );
    let function = &artifact.ir.functions[0];
    let conversion = function
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|op| matches!(op.kind, OperationKind::NumericConvert { .. }))
        .unwrap();
    let OperationKind::NumericConvert {
        error,
        cleanup_chain,
        failure,
        ..
    } = &conversion.kind
    else {
        unreachable!()
    };
    assert_ne!(error.target, function.failure);
    assert_eq!(error.cleanup_chain.len(), 1);
    assert_eq!(cleanup_chain.len(), 2);
    assert_eq!(*failure, function.failure);
    assert_eq!(error.cleanup_chain[0], cleanup_chain[0]);
    let trap = function
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|op| matches!(op.kind, OperationKind::Binary { .. }))
        .unwrap();
    assert!(
        matches!(trap.kind, OperationKind::Binary { failure: Some(target), .. } if target == function.failure)
    );
}
#[test]
fn handler_errors_go_to_outer_catch_and_return_paths_are_complete() {
    let artifact = compile(
        r#"
        fn recover() -> CInt8 {
            try {
                try { return CInt8(200); } catch inner { let other = CInt8(201); }
            } catch outer { return 0; }
            return 1;
        }
        fn main() {}
    "#,
    );
    let targets: Vec<_> = artifact.ir.functions[0]
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter_map(|op| {
            if let OperationKind::NumericConvert { error, .. } = &op.kind {
                Some(error.target)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(targets.len(), 2);
    assert_ne!(targets[0], targets[1]);
}
#[test]
fn verifier_rejects_noncatch_error_targets_and_trap_recovery() {
    let artifact =
        compile("fn main() { try { let x = CInt8(200); let trap = 1 / 0; } catch e {} }");
    for mutation in 0..2 {
        let mut ir = artifact.ir.clone();
        let target = ir.functions[0]
            .blocks
            .iter()
            .find(|b| {
                b.operations
                    .first()
                    .is_some_and(|op| matches!(op.kind, OperationKind::CatchErrors { .. }))
            })
            .unwrap()
            .id;
        let operation = ir.functions[0]
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.operations)
            .find(|op| {
                if mutation == 0 {
                    matches!(op.kind, OperationKind::NumericConvert { .. })
                } else {
                    matches!(op.kind, OperationKind::Binary { .. })
                }
            })
            .unwrap();
        if let OperationKind::NumericConvert { error, .. } = &mut operation.kind {
            error.target = rinzu_compiler::BlockId(0);
        }
        if let OperationKind::Binary { failure, .. } = &mut operation.kind {
            *failure = Some(target);
        }
        assert!(verify_module(&ir).is_err());
    }
}

#[test]
fn error_message_uses_an_explicit_rooted_projection_and_verifies_its_types() {
    let artifact = compile(
        r#"
fn message(error: Error) -> String { let number = 1; return error.message; }
fn main() { try { let bad = CInt8(200); } catch [error: Error, ...errorRest] { let text = message(error); } }
"#,
    );
    assert!(verify_module(&artifact.ir).is_ok());
    let function = artifact
        .ir
        .functions
        .iter()
        .find(|function| function.name == "message")
        .unwrap();
    assert!(!function.blocks.iter().flat_map(|block| &block.operations).any(|operation| matches!(&operation.kind, OperationKind::LoadLocal { name } if name == "error.message")));
    let mut malformed = artifact.ir.clone();
    let function = malformed
        .functions
        .iter_mut()
        .find(|function| function.name == "message")
        .unwrap();
    let number = function
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|operation| {
            operation
                .result
                .as_ref()
                .is_some_and(|value| value.ty == Type::Int)
        })
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .id;
    let projection = function
        .blocks
        .iter_mut()
        .flat_map(|block| &mut block.operations)
        .find(|operation| matches!(operation.kind, OperationKind::ErrorMessage { .. }))
        .unwrap();
    let OperationKind::ErrorMessage { error } = &mut projection.kind else {
        unreachable!()
    };
    *error = number;
    assert!(
        verify_module(&malformed)
            .unwrap_err()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Error message projection"))
    );
    let mut missing_poll = artifact.ir;
    let function = missing_poll
        .functions
        .iter_mut()
        .find(|function| function.name == "message")
        .unwrap();
    for block in &mut function.blocks {
        if let Some(index) = block
            .operations
            .iter()
            .position(|operation| matches!(operation.kind, OperationKind::ErrorMessage { .. }))
        {
            block.operations.remove(index - 1);
            break;
        }
    }
    assert!(
        verify_module(&missing_poll)
            .unwrap_err()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Error message projection"))
    );
}
