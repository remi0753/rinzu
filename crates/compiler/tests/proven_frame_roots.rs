use rinzu_compiler::{
    FileId, FrontendArtifact, IrFunction, OperationKind as O, SourceFile, compile_source,
    proven_frame_roots,
};
fn fixture() -> FrontendArtifact {
    compile_source(&SourceFile::new(
        FileId(98),
        "proven-roots.rinzu",
        r#"
class Cell { value:ByteArray }
native { fn tick()->Int symbol "tick"; }
fn identity(value:ByteArray)->ByteArray{return value;}
fn owned(seed:String)->ByteArray {
    let left=utf8Encode(seed);
    let node=Cell(value:bytesConcat(left,utf8Encode("right")));
    return bytesConcat(node.value,left);
}
fn boundaries(seed:String)->ByteArray {
    let held=utf8Encode(seed);
    let after=identity(held);
    let nativeResult=tick();
    return bytesConcat(after,held);
}
fn branch(seed:String,flag:Bool)->ByteArray {
    let held=utf8Encode(seed);
    if flag {return bytesConcat(held,utf8Encode("yes"));}
    return bytesConcat(held,utf8Encode("no"));
}
fn loop(seed:ByteArray,flag:Bool)->ByteArray {
    var held=seed; var i=0;
    while i<3 {if flag {held=bytesConcat(held,utf8Encode("x"));}i=i+1;}
    return bytesConcat(held,seed);
}
async fn child()->ByteArray{return utf8Encode("child");}
fn cleanup()->ByteArray{try{return utf8Encode("body");}finally{let value=utf8Encode("cleanup");}}
fn caught()->ByteArray{try{return utf8Encode("body");}catch e {return utf8Encode("caught");}}
fn main(){}
"#,
    ))
    .unwrap()
}
fn function<'a>(artifact: &'a FrontendArtifact, name: &str) -> &'a IrFunction {
    artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap()
}
fn checked_entry(artifact: &FrontendArtifact, name: &str) {
    let f = function(artifact, name);
    let first = f.blocks.iter().find(|b| b.id == f.entry).unwrap().operations.iter()
        .enumerate().find(|(_, o)| matches!(&o.kind, O::GcSafepoint {stack_map,..} if !stack_map.roots.is_empty())).unwrap().0;
    assert!(
        !proven_frame_roots(&artifact.ir)
            .get(name)
            .is_some_and(|p| p.contains(&(f.entry, first)))
    );
}
#[test]
fn owned_chains_reduce_whole_vectors_without_mutating_semantic_ir() {
    let a = fixture();
    let before = format!("{:?}", a.ir);
    let plans = proven_frame_roots(&a.ir);
    assert!(plans["owned"].len() >= 3);
    for (name, plan) in plans {
        let f = function(&a, &name);
        for (block, index) in plan {
            assert!(
                matches!(&f.blocks.iter().find(|b| b.id == block).unwrap().operations[index].kind,
                O::GcSafepoint {stack_map,..} if !stack_map.roots.is_empty())
            );
        }
    }
    assert_eq!(before, format!("{:?}", a.ir));
}
#[test]
fn parameters_are_unknown_even_when_their_type_is_managed() {
    let a = fixture();
    for name in ["identity", "owned", "boundaries", "branch", "loop"] {
        checked_entry(&a, name);
    }
}
#[test]
fn retained_caller_words_and_closed_returns_survive_managed_and_native_calls() {
    let a = fixture();
    let f = function(&a, "boundaries");
    let plans = proven_frame_roots(&a.ir);
    let mut observed = 0;
    for block in &f.blocks {
        for (index, op) in block.operations.iter().enumerate() {
            if matches!(
                &op.kind,
                O::ManagedCall { .. } | O::ReleaseNativeCall { .. }
            ) && let Some((next, _)) = block
                .operations
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(_, o)| matches!(o.kind, O::GcSafepoint { .. }))
            {
                assert!(plans["boundaries"].contains(&(block.id, next)));
                observed += 1;
            }
        }
    }
    assert!(observed >= 2);
}
#[test]
fn branch_and_loop_predecessors_contribute_valid_local_facts() {
    let a = fixture();
    let plans = proven_frame_roots(&a.ir);
    for name in ["branch", "loop"] {
        let f = function(&a, name);
        assert!(plans[name].iter().any(|(bid, _)| *bid != f.entry), "{name}");
    }
}
#[test]
fn an_unknown_store_on_a_backedge_prevents_optimistic_loop_certification() {
    let mut a = fixture();
    let original = proven_frame_roots(&a.ir)["loop"].clone();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "loop")
            .unwrap();
    let mut changed = 0;
    for block in &mut f.blocks {
        for op in &mut block.operations {
            if let O::DataOperation { .. } = &op.kind
                && op
                    .result
                    .as_ref()
                    .is_some_and(|v| v.ty.is_managed_reference())
            {
                // A deliberately unknown definition, never emitted. This
                // negative IR probe models an unproved producer feeding
                // the loop local without granting it a typed fact.
                op.kind = O::LoadLocal {
                    name: "proof-unknown".into(),
                };
                changed += 1;
            }
        }
    }
    assert!(changed > 0);
    let after = proven_frame_roots(&a.ir)["loop"].clone();
    assert!(!original.is_subset(&after));
}
#[test]
fn replacing_a_vector_drops_unretained_word_facts_before_a_call() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "boundaries")
            .unwrap();
    let b = f.blocks.iter_mut().find(|b| b.id == f.entry).unwrap();
    let call = b
        .operations
        .iter()
        .position(|o| matches!(&o.kind, O::ManagedCall {target,..} if target == "identity"))
        .unwrap();
    let before = (0..call)
        .rev()
        .find(|i| matches!(b.operations[*i].kind, O::GcSafepoint { .. }))
        .unwrap();
    let removed = match &mut b.operations[before].kind {
        O::GcSafepoint { stack_map, .. } => {
            let removed = stack_map.roots.clone();
            stack_map.roots.clear();
            removed
        }
        _ => unreachable!(),
    };
    assert!(!removed.is_empty());
    let after = b
        .operations
        .iter()
        .enumerate()
        .skip(call + 1)
        .find(|(_, o)| matches!(o.kind, O::GcSafepoint { .. }))
        .unwrap()
        .0;
    let entry = f.entry;
    // An omitted current vector cannot keep the old caller word alive. The
    // candidate analyzer must demand checking at the later publication.
    assert!(!proven_frame_roots(&a.ir)["boundaries"].contains(&(entry, after)));
}
#[test]
fn a_nonterminal_error_edge_excludes_the_activation() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "boundaries")
            .unwrap();
    let entry = f.entry;
    let op = f
        .blocks
        .iter_mut()
        .flat_map(|b| &mut b.operations)
        .find(|o| matches!(o.kind, O::ManagedCall { .. }))
        .unwrap();
    if let O::ManagedCall { error, .. } = &mut op.kind {
        error.target = entry;
    }
    assert!(!proven_frame_roots(&a.ir).contains_key("boundaries"));
}
#[test]
fn async_catch_and_finally_keep_full_validation() {
    let a = fixture();
    let plans = proven_frame_roots(&a.ir);
    for name in ["child", "cleanup", "caught"] {
        assert!(!plans.contains_key(name), "{name}");
    }
}
