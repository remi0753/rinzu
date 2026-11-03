use rinzu_compiler::{
    FileId, FrontendArtifact, OperationKind as O, SourceFile, StackRoot, Type, compile_source,
    proven_frame_memberships,
};

fn fixture() -> FrontendArtifact {
    compile_source(&SourceFile::new(
        FileId(97),
        "memberships.rinzu",
        r#"
class Cell { value:ByteArray }
native { fn tick()->Int symbol "tick"; }
fn identity(seed:ByteArray)->ByteArray{return seed;}
fn branch(seed:ByteArray,flag:Bool)->ByteArray {
    let held=bytesConcat(seed,utf8Encode("held"));
    if flag {return bytesConcat(seed,held);}
    return bytesConcat(seed,held);
}
async fn child(seed:ByteArray)->ByteArray{return bytesConcat(seed,utf8Encode("child"));}
async fn work(seed:ByteArray)->ByteArray {
    let held=bytesConcat(seed,utf8Encode("held"));
    let received=await child(held);
    return bytesConcat(received,held);
}
fn nativeBoundary(seed:ByteArray)->ByteArray {
    let held=bytesConcat(seed,utf8Encode("held"));
    let code=tick();
    return bytesConcat(held,seed);
}
fn caught(seed:ByteArray)->ByteArray {
    try {return bytesConcat(seed,utf8Encode("try"));}
    catch error {return bytesConcat(seed,utf8Encode("catch"));}
}
fn loop(seed:ByteArray,flag:Bool)->ByteArray {
    var held=seed;var i=0;
    while i<3 {if flag {held=bytesConcat(held,utf8Encode("next"));}i=i+1;}
    return bytesConcat(held,seed);
}
fn main(){}
"#,
    ))
    .unwrap()
}

#[test]
fn current_vectors_and_ir_stay_unchanged_across_async_and_catch_analysis() {
    let a = fixture();
    let before = format!("{:?}", a.ir);
    let plans = proven_frame_memberships(&a.ir);
    assert!(!plans["work"].is_empty());
    assert!(!plans["caught"].is_empty());
    for (name, plan) in &plans {
        let f = a.ir.functions.iter().find(|f| &f.name == name).unwrap();
        for ((bid, index), mask) in plan {
            assert_ne!(*mask, 0);
            assert!(matches!(
                f.blocks.iter().find(|b| b.id == *bid).unwrap().operations[*index].kind,
                O::GcSafepoint { .. }
            ));
        }
    }
    assert_eq!(before, format!("{:?}", a.ir));
}

#[test]
fn managed_parameters_do_not_create_entry_authority() {
    let a = fixture();
    let plans = proven_frame_memberships(&a.ir);
    for f in
        a.ir.functions
            .iter()
            .filter(|f| !f.parameter_types.is_empty())
    {
        let b = f.blocks.iter().find(|b| b.id == f.entry).unwrap();
        let index = b
            .operations
            .iter()
            .position(|o| matches!(o.kind, O::GcSafepoint { .. }))
            .unwrap();
        assert!(!plans[&f.name].contains_key(&(b.id, index)), "{}", f.name);
    }
}

#[test]
fn exceptional_entry_never_imports_a_normal_predecessors_word() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "branch")
            .unwrap();
    let target = f
        .blocks
        .iter()
        .find_map(|b| {
            if let rinzu_compiler::Terminator::Branch { then_target, .. } = b.terminator {
                Some(then_target)
            } else {
                None
            }
        })
        .unwrap();
    let mut checkpoint = f
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .find(|o| matches!(o.kind, O::GcSafepoint { .. }))
        .unwrap()
        .clone();
    checkpoint.kind = O::GcSafepoint {
        reason: rinzu_compiler::PollReason::ManagedCall,
        stack_map: rinzu_compiler::StackMap {
            roots: vec![StackRoot::Local {
                name: "seed".into(),
                ty: Type::Bytes,
            }],
        },
    };
    f.blocks
        .iter_mut()
        .find(|b| b.id == target)
        .unwrap()
        .operations
        .insert(0, checkpoint.clone());
    assert_eq!(
        proven_frame_memberships(&a.ir)["branch"].get(&(target, 0)),
        Some(&1)
    );
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "branch")
            .unwrap();
    checkpoint.kind = O::EnterTry { cleanup: target };
    f.blocks
        .iter_mut()
        .find(|b| b.id == f.entry)
        .unwrap()
        .operations
        .push(checkpoint);
    assert!(!proven_frame_memberships(&a.ir)["branch"].contains_key(&(target, 0)));
}

#[test]
fn await_and_unknown_effects_discard_unpublished_results() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "work")
            .unwrap();
    let (bid, index) = f
        .blocks
        .iter()
        .find_map(|b| {
            b.operations
                .iter()
                .enumerate()
                .find_map(|(i, o)| matches!(o.kind, O::Await { .. }).then_some((b.id, i)))
        })
        .unwrap();
    let b = f.blocks.iter_mut().find(|b| b.id == bid).unwrap();
    let mut probe = b
        .operations
        .iter()
        .find(|o| matches!(o.kind, O::GcSafepoint { .. }))
        .unwrap()
        .clone();
    probe.kind = O::GcSafepoint {
        reason: rinzu_compiler::PollReason::Await,
        stack_map: rinzu_compiler::StackMap {
            roots: vec![StackRoot::Local {
                name: "held".into(),
                ty: Type::Bytes,
            }],
        },
    };
    b.operations.insert(index + 1, probe);
    assert!(!proven_frame_memberships(&a.ir)["work"].contains_key(&(bid, index + 1)));
}

#[test]
fn partial_selector_is_bounded_and_large_vectors_keep_fallback() {
    let mut a = fixture();
    let f =
        a.ir.functions
            .iter_mut()
            .find(|f| f.name == "identity")
            .unwrap();
    let entry = f.entry;
    let b = f.blocks.iter_mut().find(|b| b.id == entry).unwrap();
    let mut probe = b
        .operations
        .iter()
        .find(|o| matches!(o.kind, O::GcSafepoint { .. }))
        .unwrap()
        .clone();
    let root = StackRoot::Local {
        name: "seed".into(),
        ty: Type::Bytes,
    };
    probe.kind = O::GcSafepoint {
        reason: rinzu_compiler::PollReason::ManagedCall,
        stack_map: rinzu_compiler::StackMap {
            roots: vec![root.clone(); 64],
        },
    };
    let i = b
        .operations
        .iter()
        .position(|o| matches!(o.kind, O::GcSafepoint { .. }))
        .unwrap()
        + 1;
    b.operations.insert(i, probe.clone());
    probe.kind = O::GcSafepoint {
        reason: rinzu_compiler::PollReason::ManagedCall,
        stack_map: rinzu_compiler::StackMap {
            roots: vec![root; 65],
        },
    };
    b.operations.insert(i + 1, probe);
    let plans = proven_frame_memberships(&a.ir);
    assert_eq!(plans["identity"].get(&(entry, i)), Some(&u64::MAX));
    assert!(!plans["identity"].contains_key(&(entry, i + 1)));
}
