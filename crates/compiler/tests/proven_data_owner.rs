use rinzu_compiler::{
    BlockId, DataOperation, FileId, FrontendArtifact, IrFunction, Operation, OperationKind as O,
    SourceFile, Terminator, compile_source, proven_data_owner_operations, virtual_bytes_options,
};

fn fixture() -> FrontendArtifact {
    compile_source(&SourceFile::new(FileId(98), "proven-owner.rinzu", r#"
class Cell { value:ByteArray }
native { fn tick()->Int symbol "tick"; }
fn identity(value:ByteArray)->ByteArray{return value;}
fn nativeChild(value:ByteArray)->ByteArray{let n=tick();return value;}
fn owned(seed:String)->ByteArray {
    let left=utf8Encode(seed);
    let node=Cell(value:bytesConcat(left,utf8Encode("right")));
    return bytesConcat(node.value,left);
}
fn boundaries(seed:String)->ByteArray {
    let held=utf8Encode(seed);let a=identity(held);let b=nativeChild(a);
    let nativeResult=tick();return bytesConcat(b,held);
}
fn merge(seed:ByteArray,flag:Bool)->ByteArray {
    var held=seed;if flag {held=seed;}else{held=seed;}
    return bytesConcat(held,seed);
}
fn loop(seed:ByteArray)->ByteArray {
    var held=seed;var i=0;while i<3 {held=bytesConcat(held,seed);i=i+1;}
    return bytesConcat(held,seed);
}
async fn child()->ByteArray{return utf8Encode("child");}
async fn resumed()->ByteArray{let held=utf8Encode("held");let next=await child();return bytesConcat(held,next);}
fn cleanup()->ByteArray{try{return utf8Encode("body");}finally{let value=utf8Encode("cleanup");}}
fn caught()->ByteArray{try{let n=tick();return utf8Encode("body");}catch e{return utf8Encode("caught");}}
fn sliced(value:ByteArray)->ByteArray {
    switch bytesSlice(value,1,3) {
        case Option.Some(piece): {return bytesConcat(piece,utf8Encode("x"));}
        case Option.None: {return byteArray(b"");}
    }
}
fn main(){}
"#)).unwrap()
}
fn function<'a>(a: &'a FrontendArtifact, name: &str) -> &'a IrFunction {
    a.ir.functions.iter().find(|f| f.name == name).unwrap()
}
fn first_checked(f: &IrFunction, block: BlockId) -> Option<usize> {
    f.blocks
        .iter()
        .find(|b| b.id == block)
        .unwrap()
        .operations
        .iter()
        .position(|o| {
            matches!(
                o.kind,
                O::GcSafepoint { .. }
                    | O::Construct { .. }
                    | O::DataOperation { .. }
                    | O::Field { .. }
                    | O::EnumPayload { .. }
            )
        })
}
fn successors(t: &Terminator) -> Vec<BlockId> {
    match t {
        Terminator::Goto { target, .. } => vec![*target],
        Terminator::Branch {
            then_target,
            else_target,
            ..
        } => vec![*then_target, *else_target],
        Terminator::Switch { cases, .. } => cases.iter().map(|(_, b)| *b).collect(),
        _ => Vec::new(),
    }
}
#[test]
fn owned_chains_and_cfg_facts_leave_semantic_ir_unchanged() {
    let a = fixture();
    let before = format!("{:?}", a.ir);
    let plans = proven_data_owner_operations(&a.ir);
    assert!(plans["owned"].len() >= 3);
    for name in ["merge", "loop"] {
        let f = function(&a, name);
        assert!(plans[name].iter().any(|(b, _)| *b != f.entry), "{name}");
    }
    assert_eq!(before, format!("{:?}", a.ir));
}
#[test]
fn fresh_entry_checks_remain_even_for_initialized_parameter_types() {
    let a = fixture();
    let plans = proven_data_owner_operations(&a.ir);
    for f in &a.ir.functions {
        if let Some(i) = first_checked(f, f.entry) {
            // Parameters and scope metadata preceding this operation do not
            // establish current-thread authority, even in async activations.
            if !f
                .blocks
                .iter()
                .find(|b| b.id == f.entry)
                .unwrap()
                .operations[..i]
                .iter()
                .any(|o| {
                    matches!(
                        o.kind,
                        O::Constant(
                            rinzu_compiler::ConstantValue::String(_)
                                | rinzu_compiler::ConstantValue::Bytes(_)
                        ) | O::BytesPad { .. }
                            | O::StoreField { .. }
                    )
                })
            {
                assert!(!plans[&f.name].contains(&(f.entry, i)), "{}", f.name);
            }
        }
    }
}
#[test]
fn one_unknown_predecessor_prevents_join_certification() {
    let mut a = fixture();
    let original = proven_data_owner_operations(&a.ir)["merge"].clone();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "merge")
            .unwrap();
    let (join, index, pred) = f
        .blocks
        .iter()
        .find_map(|b| {
            let i = first_checked(f, b.id)?;
            let preds: Vec<_> = f
                .blocks
                .iter()
                .filter(|p| successors(&p.terminator).contains(&b.id))
                .map(|p| p.id)
                .collect();
            (preds.len() >= 2 && original.contains(&(b.id, i))).then(|| (b.id, i, preds[0]))
        })
        .expect("fixture must contain a certified real join");
    f.blocks
        .iter_mut()
        .find(|b| b.id == pred)
        .unwrap()
        .operations
        .push(Operation {
            result: None,
            kind: O::AsyncStart,
            unwind: None,
            range: f.range,
        });
    assert!(!proven_data_owner_operations(&a.ir)["merge"].contains(&(join, index)));
}
#[test]
fn an_unknown_loop_backedge_cannot_self_justify_authority() {
    let mut a = fixture();
    let original = proven_data_owner_operations(&a.ir)["loop"].clone();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "loop")
            .unwrap();
    let (join, index, pred) = f
        .blocks
        .iter()
        .find_map(|b| {
            let i = first_checked(f, b.id)?;
            let pred = f
                .blocks
                .iter()
                .find(|p| p.id.0 > b.id.0 && successors(&p.terminator).contains(&b.id))?;
            original.contains(&(b.id, i)).then_some((b.id, i, pred.id))
        })
        .expect("fixture must contain a certified loop header");
    f.blocks
        .iter_mut()
        .find(|b| b.id == pred)
        .unwrap()
        .operations
        .push(Operation {
            result: None,
            kind: O::AsyncStart,
            unwind: None,
            range: f.range,
        });
    assert!(!proven_data_owner_operations(&a.ir)["loop"].contains(&(join, index)));
}
#[test]
fn error_targets_never_inherit_a_normal_predecessor_fact() {
    let mut a = fixture();
    let old = proven_data_owner_operations(&a.ir)["merge"].clone();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "merge")
            .unwrap();
    let (target, index) = f
        .blocks
        .iter()
        .filter(|b| b.id != f.entry)
        .find_map(|b| {
            let i = first_checked(f, b.id)?;
            old.contains(&(b.id, i)).then_some((b.id, i))
        })
        .unwrap();
    let row = f
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| matches!(o.kind, O::DataOperation { .. }))
        .unwrap();
    if let O::DataOperation { error, .. } = &mut row.kind {
        error.target = target;
    }
    assert!(!proven_data_owner_operations(&a.ir)["merge"].contains(&(target, index)));
}
#[test]
fn native_and_transitive_native_calls_keep_the_next_checkpoint_checked() {
    let a = fixture();
    let f = function(&a, "boundaries");
    let plan = proven_data_owner_operations(&a.ir)["boundaries"].clone();
    let mut observed = std::collections::HashSet::new();
    for b in &f.blocks {
        for (i, o) in b.operations.iter().enumerate() {
            if matches!(&o.kind,O::ManagedCall{target,..}if target=="nativeChild")
                || matches!(
                    o.kind,
                    O::ReleaseNativeCall { .. } | O::NativeCall { .. } | O::SourceNativeCall { .. }
                )
            {
                for (j, next) in b.operations.iter().enumerate().skip(i + 1) {
                    if matches!(
                        next.kind,
                        O::GcSafepoint { .. }
                            | O::Construct { .. }
                            | O::DataOperation { .. }
                            | O::Field { .. }
                            | O::EnumPayload { .. }
                    ) {
                        assert!(!plan.contains(&(b.id, j)));
                        observed.insert(matches!(o.kind, O::ManagedCall { .. }));
                        break;
                    }
                    // A real intervening producer check ends the negative
                    // region. Scalar constants/scope metadata do not.
                    if matches!(
                        next.kind,
                        O::Constant(
                            rinzu_compiler::ConstantValue::String(_)
                                | rinzu_compiler::ConstantValue::Bytes(_)
                        ) | O::BytesPad { .. }
                            | O::StoreField { .. }
                    ) {
                        break;
                    }
                }
            }
        }
    }
    assert_eq!(
        observed,
        std::collections::HashSet::from([false, true]),
        "must cover direct and transitive boundaries"
    );
}

#[test]
fn a_closed_synchronous_call_preserves_an_existing_owner_fact() {
    let a = fixture();
    let f = function(&a, "boundaries");
    let plan = proven_data_owner_operations(&a.ir)["boundaries"].clone();
    let mut observed = 0;
    for block in &f.blocks {
        for (i, op) in block.operations.iter().enumerate() {
            if matches!(&op.kind, O::ManagedCall { target, .. } if target == "identity") {
                let next = block
                    .operations
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find(|(_, op)| matches!(op.kind, O::GcSafepoint { .. }))
                    .unwrap()
                    .0;
                assert!(plan.contains(&(block.id, next)));
                observed += 1;
            }
        }
    }
    assert_eq!(observed, 1);
}
#[test]
fn await_resume_requires_a_new_successful_check() {
    let a = fixture();
    let f = function(&a, "resumed");
    let plan = proven_data_owner_operations(&a.ir)["resumed"].clone();
    let mut observed = 0;
    for b in &f.blocks {
        for (i, o) in b
            .operations
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o.kind, O::Await { .. }))
        {
            let _ = o;
            if let Some((j, _)) = b
                .operations
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, o)| matches!(o.kind, O::GcSafepoint { .. }))
            {
                assert!(!plan.contains(&(b.id, j)));
                observed += 1;
            }
        }
    }
    assert!(observed > 0);
}
#[test]
fn virtual_option_projection_does_not_claim_a_nonexistent_runtime_check() {
    let a = fixture();
    let f = function(&a, "sliced");
    let virtuals = virtual_bytes_options(f);
    assert!(!virtuals.is_empty());
    let plan = proven_data_owner_operations(&a.ir)["sliced"].clone();
    let mut observed = 0;
    for b in &f.blocks {
        for (i, o) in b.operations.iter().enumerate() {
            if matches!(&o.kind,O::EnumPayload{operand,..}if virtuals.contains(operand)) {
                assert!(!plan.contains(&(b.id, i)));
                observed += 1;
            }
        }
    }
    assert!(observed > 0);
    assert!(
        f.blocks
            .iter()
            .any(|b| b.operations.iter().enumerate().any(|(i, o)| matches!(
                o.kind,
                O::DataOperation {
                    kind: DataOperation::BytesConcat,
                    ..
                }
            ) && plan.contains(&(b.id, i))))
    );
}
