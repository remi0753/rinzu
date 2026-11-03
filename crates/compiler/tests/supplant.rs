use rinzu_compiler::{
    DiagnosticCode, FileId, NativeEntryDomain, OperationKind, SourceFile, compile_source,
    verify_module,
};

fn source(body: &str) -> SourceFile {
    SourceFile::new(
        FileId(0),
        "supplant.rz",
        format!(
            r#"
        native {{ supplant fn run(x: Int) -> Int symbol "run"; }}
        fn identity(x: Int) -> Int {{ return x; }}
        {body}
    "#
        ),
    )
}
#[test]
fn direct_await_alias_parameter_and_parentheses_keep_protocol() {
    let artifact = compile_source(&source(
        r#"
        async fn apply(f: supplant fn(Int) -> Int) -> Int { return await f(1); }
        async fn wrapper() -> Int { return await (run(1)); }
        async fn main() {
            try {
            let alias: supplant fn(Int) -> Int = run;
            let result = await alias(2);
            let other = await apply(alias);
            let last = await wrapper();
            } catch errors {}
        }
    "#,
    ))
    .unwrap();
    assert_eq!(artifact.checked.supplant_calls.len(), 3);
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .filter(|f| f.declaration.is_async)
            .all(|f| f.facts.requires_owner && !f.facts.worker_safe && !f.facts.cleanup_safe)
    );
    for function in &artifact.ir.functions {
        for block in &function.blocks {
            for (index, operation) in block.operations.iter().enumerate() {
                if matches!(operation.kind, OperationKind::ReserveSupplant { .. }) {
                    assert!(matches!(
                        block.operations[index + 1].kind,
                        OperationKind::StartSupplant {
                            entry_domain: NativeEntryDomain::ProcessMain,
                            suspend_before_entry: true,
                            ..
                        }
                    ));
                    assert!(matches!(
                        block.operations[index + 2].kind,
                        OperationKind::SettleSupplant { release: true, .. }
                    ));
                }
            }
        }
    }
}
#[test]
fn owner_requirement_reaches_recursive_and_indirect_wrappers() {
    let artifact = compile_source(&source(
        r#"
        async fn first(x: Bool) -> Int { if x { return await second(false); } return await run(1); }
        async fn second(x: Bool) -> Int { return await first(x); }
        async fn main() { try { let alias = second; let result = await alias(true); } catch errors {} }
    "#,
    ))
    .unwrap();
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .filter(|f| f.declaration.is_async)
            .all(|f| f.facts.requires_owner)
    );
}
#[test]
fn missing_direct_await_effect_erasure_and_worker_calls_are_rejected() {
    for body in [
        "async fn main() { let pending = run(1); }",
        "async fn main() { let x = await identity(run(1)); }",
        "async fn main() { let alias = run; let x = alias(1); }",
        "async fn main() { let alias: fn(Int) -> Int = run; }",
        "fn main() { let result = await run(1); }",
        "async fn wrapper() -> Int { return await run(1); } fn worker() { let pending = wrapper(); } async fn main() { await spawn worker(); }",
    ] {
        assert!(compile_source(&source(body)).is_err(), "accepted {body}");
    }
    let errors = compile_source(&source("async fn main() { let x = run(1); }")).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::InvalidAwait)
    );
}
#[test]
fn verifier_rejects_missing_entry_release_roots_and_wrong_domain() {
    let artifact = compile_source(&source(
        "async fn main() { try { let x = await run(1); } catch errors {} }",
    ))
    .unwrap();
    for mutation in 0..5 {
        let mut ir = artifact.ir.clone();
        let block = ir
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .find(|b| {
                b.operations
                    .iter()
                    .any(|op| matches!(op.kind, OperationKind::ReserveSupplant { .. }))
            })
            .unwrap();
        let index = block
            .operations
            .iter()
            .position(|op| matches!(op.kind, OperationKind::ReserveSupplant { .. }))
            .unwrap();
        match mutation {
            0 => {
                block.operations.remove(index + 1);
            }
            1 => {
                if let OperationKind::SettleSupplant { release, .. } =
                    &mut block.operations[index + 2].kind
                {
                    *release = false;
                }
            }
            2 => {
                if let OperationKind::ReserveSupplant { retained, .. } =
                    &mut block.operations[index].kind
                {
                    retained.clear();
                }
            }
            3 => {
                if let OperationKind::StartSupplant { entry_domain, .. } =
                    &mut block.operations[index + 1].kind
                {
                    *entry_domain = NativeEntryDomain::CurrentCaller;
                }
            }
            4 => {
                let settle = block.operations[index + 2].clone();
                block.operations.push(settle);
            }
            _ => unreachable!(),
        }
        assert!(verify_module(&ir).is_err(), "accepted mutation {mutation}");
    }
}
