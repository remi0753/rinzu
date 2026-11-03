use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source, verify_module};

#[test]
fn fresh_direct_await_is_consumed_but_named_and_returned_futures_keep_aliases() {
    let source = SourceFile::new(
        FileId(1),
        "consume.rinzu",
        "async fn child()->Int{return 7;} async fn main(){let f=child();let a=await f;let b=await f;let c=await child();}",
    );
    let artifact = compile_source(&source).unwrap();
    let awaits = artifact
        .ir
        .functions
        .iter()
        .filter(|f| f.name == "main")
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter_map(|op| match op.kind {
            OperationKind::Await { consume, .. } => Some(consume),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(awaits, vec![false, false, true]);
    let source = SourceFile::new(
        FileId(2),
        "returned.rinzu",
        "async fn child()->Int{return 7;} fn cached(f:Future<Int>)->Future<Int>{return f;} async fn main(){let f=child();let a=await cached(f);let b=await f;}",
    );
    let artifact = compile_source(&source).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .all(|op| !matches!(op.kind, OperationKind::Await { consume: true, .. }))
    );
}
#[test]
fn forged_consumption_of_a_named_future_is_rejected_by_ir_verifier() {
    let source = SourceFile::new(
        FileId(1),
        "forged.rinzu",
        "async fn child()->Int{return 7;} async fn main(){let f=child();let a=await f;let b=await f;}",
    );
    let mut module = compile_source(&source).unwrap().ir;
    for operation in module
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
    {
        if let OperationKind::Await { consume, .. } = &mut operation.kind {
            *consume = true;
        }
    }
    let errors = verify_module(&module).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("consum") || e.message.contains("fresh")),
        "{errors:?}"
    );
}
