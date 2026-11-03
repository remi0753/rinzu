use rinzu_compiler::{
    FileId, FrontendArtifact, OperationKind as O, SourceFile, compile_source,
    fresh_return_consuming_awaits,
};

fn fixture() -> FrontendArtifact {
    compile_source(&SourceFile::new(FileId(99), "fresh-future.rinzu", r#"
async fn leaf()->ByteArray{return utf8Encode("leaf");}
fn direct()->Future<ByteArray>{return leaf();}
fn chain()->Future<ByteArray>{return direct();}
fn branch(flag:Bool)->Future<ByteArray>{if flag{return direct();}return leaf();}
fn identity(value:Future<ByteArray>)->Future<ByteArray>{return value;}
fn stored()->Future<ByteArray>{let value=leaf();return value;}
fn cleanup()->Future<ByteArray>{try{return leaf();}finally{let value=utf8Encode("cleanup");}}
async fn positive(){let a=await direct();let b=await chain();let c=await branch(true);}
async fn aliases(){let pending=leaf();let a=await identity(pending);let b=await pending;let c=await stored();let d=await cleanup();}
async fn caught(){try{let a=await direct();}catch errors{}}
async fn main(){await positive();await aliases();await caught();}
"#)).unwrap()
}

#[test]
fn direct_chained_and_all_fresh_branch_returns_are_consumed() {
    let a = fixture();
    assert_eq!(fresh_return_consuming_awaits(&a.ir)["positive"].len(), 3);
}

#[test]
fn parameters_locals_reawait_and_cleanup_wrappers_stay_checked() {
    let a = fixture();
    assert!(!fresh_return_consuming_awaits(&a.ir).contains_key("aliases"));
}

#[test]
fn caller_catch_does_not_weaken_the_wrappers_return_proof() {
    let a = fixture();
    assert_eq!(fresh_return_consuming_awaits(&a.ir)["caught"].len(), 1);
}

#[test]
fn semantic_ir_flags_and_root_vectors_are_unchanged() {
    let a = fixture();
    let before = format!("{:?}", a.ir);
    let plan = fresh_return_consuming_awaits(&a.ir);
    for f in &a.ir.functions {
        if let Some(sites) = plan.get(&f.name) {
            for (bid, index) in sites {
                let b = f.blocks.iter().find(|b| b.id == *bid).unwrap();
                assert!(matches!(
                    b.operations[*index].kind,
                    O::Await { consume: false, .. }
                ));
            }
        }
    }
    assert_eq!(before, format!("{:?}", a.ir));
}

#[test]
fn unknown_return_on_one_branch_excludes_the_whole_wrapper() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "branch")
            .unwrap();
    let op = f
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|op| matches!(&op.kind,O::ManagedCall{target,..} if target=="direct"))
        .unwrap();
    op.kind = O::LoadLocal {
        name: "unknown-future".into(),
    };
    assert_eq!(fresh_return_consuming_awaits(&a.ir)["positive"].len(), 2);
}

#[test]
fn additional_future_operand_use_prevents_consume() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "positive")
            .unwrap();
    let b = f
        .blocks
        .iter_mut()
        .find(|b| {
            b.operations
                .iter()
                .any(|o| matches!(o.kind, O::Await { .. }))
        })
        .unwrap();
    let index = b
        .operations
        .iter()
        .position(|o| matches!(o.kind, O::Await { .. }))
        .unwrap();
    let O::Await { future, .. } = b.operations[index].kind else {
        unreachable!()
    };
    let mut extra = b.operations[index].clone();
    extra.result = None;
    extra.kind = O::StoreLocal {
        name: "escaped".into(),
        value: future,
    };
    b.operations.insert(index, extra);
    assert_eq!(fresh_return_consuming_awaits(&a.ir)["positive"].len(), 2);
}

#[test]
fn recursive_freshness_is_not_assumed() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "direct")
            .unwrap();
    let op = f
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| matches!(o.kind, O::ManagedCall { .. }))
        .unwrap();
    if let O::ManagedCall { target, .. } = &mut op.kind {
        *target = "chain".into();
    }
    assert!(!fresh_return_consuming_awaits(&a.ir).contains_key("positive"));
}

#[test]
fn escaping_a_returned_future_prevents_freshness() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "direct")
            .unwrap();
    let b = f
        .blocks
        .iter_mut()
        .find(|b| {
            b.operations
                .iter()
                .any(|o| matches!(o.kind, O::ManagedCall { .. }))
        })
        .unwrap();
    let index = b
        .operations
        .iter()
        .position(|o| matches!(o.kind, O::ManagedCall { .. }))
        .unwrap();
    let value = b.operations[index].result.as_ref().unwrap().id;
    let mut escape = b.operations[index].clone();
    escape.result = None;
    escape.kind = O::StoreLocal {
        name: "escaped".into(),
        value,
    };
    b.operations.insert(index + 1, escape);
    assert!(!fresh_return_consuming_awaits(&a.ir).contains_key("positive"));
}
