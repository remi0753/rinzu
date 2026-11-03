use rinzu_compiler::{
    CompilerOptions, FileId, OperationKind, SourceFile, Terminator, compile_source,
    compile_source_with_options, verify_module,
};

#[test]
fn native_record_reads_run_finally_before_root_release() {
    let source = SourceFile::new(
        FileId(0),
        "scopes.rz",
        r#"
        native { record Pair { x: Int } }
        fn main() {
            try {
                let output = Pair(x: 1);
                let result = output.x;
            } catch [error: Error, ...errorRest] { let text = error.message; }
            finally { let final_value = true; }
        }
    "#,
    );
    let mut options = CompilerOptions::default();
    options.sdk_files.insert(FileId(0));
    let artifact = compile_source_with_options(&source, &options).unwrap();
    let function = &artifact.ir.functions[0];
    let scope = function
        .scopes
        .iter()
        .find(|scope| scope.locals.iter().any(|name| name == "output"))
        .unwrap();
    let owner_scope = scope.scope;
    assert!(scope.locals.iter().any(|name| name == "output"));
    assert_eq!(scope.deferred_finally.len(), 1);
    let read = function
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|op| matches!(op.kind, OperationKind::Field { .. }))
        .unwrap();
    assert!(
        read.unwind
            .as_ref()
            .unwrap()
            .release_scopes
            .contains(&owner_scope)
    );
    assert_eq!(
        read.unwind.as_ref().unwrap().finally,
        scope.deferred_finally
    );
    let OperationKind::Field { error, .. } = &read.kind else {
        unreachable!()
    };
    let error = error
        .as_ref()
        .expect("native record read has a Busy Error edge");
    assert_eq!(error.release_scopes, vec![owner_scope]);
    assert!(error.cleanup_chain.is_empty());
}
#[test]
fn break_and_continue_only_unwind_inside_the_loop() {
    let artifact = compile_source(&SourceFile::new(
        FileId(0),
        "scopes.rz",
        r#"
        fn main() {
            try {
                while true {
                    try { break; } finally { let inner = true; }
                }
            } finally { let outer = true; }
        }
    "#,
    ))
    .unwrap();
    let edges: Vec<_> = artifact.ir.functions[0]
        .blocks
        .iter()
        .filter_map(|block| {
            if let Terminator::Goto {
                cleanup_chain,
                release_scopes,
                ..
            } = &block.terminator
                && !release_scopes.is_empty()
            {
                return Some((cleanup_chain, release_scopes));
            }
            None
        })
        .collect();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].0.len(), 1);
    assert_eq!(edges[0].1.len(), 2);
}
#[test]
fn verifier_rejects_missing_or_duplicate_scope_release() {
    let source = SourceFile::new(
        FileId(0),
        "scopes.rz",
        "fn main() { try { let x = CInt8(200); } catch error {} finally {} }",
    );
    let artifact = compile_source(&source).unwrap();
    for mutation in 0..4 {
        let mut ir = artifact.ir.clone();
        let operation = ir.functions[0].blocks.iter_mut().flat_map(|b| &mut b.operations).find(|op|
            if mutation == 3 { matches!(op.kind, OperationKind::ExitScope { ref deferred_finally, .. } if !deferred_finally.is_empty()) }
            else { matches!(op.kind, OperationKind::NumericConvert { .. }) }).unwrap();
        match mutation {
            0 => operation.unwind = None,
            1 => {
                let plan = operation.unwind.as_mut().unwrap();
                plan.release_scopes.push(plan.release_scopes[0]);
            }
            2 => {
                if let OperationKind::NumericConvert { error, .. } = &mut operation.kind {
                    error.release_scopes.clear();
                }
            }
            3 => {
                if let OperationKind::ExitScope {
                    deferred_finally, ..
                } = &mut operation.kind
                {
                    deferred_finally.clear();
                }
            }
            _ => unreachable!(),
        }
        assert!(verify_module(&ir).is_err(), "accepted mutation {mutation}");
    }
}
