use rinzu_compiler::{
    FileId, OperationKind, PollReason, SourceFile, StackRoot, compile_source, dead_local_roots,
};
fn source(text: &str) -> rinzu_compiler::FrontendArtifact {
    compile_source(&SourceFile::new(FileId(97), "local-liveness.rinzu", text)).unwrap()
}
#[test]
fn fixed_point_keeps_loop_carried_locals_and_removes_overwritten_or_dead_locals() {
    let artifact = source(
        r#"
fn looped(seed: ByteArray, flag: Bool) -> ByteArray {
    var kept=seed;
    var turn=0;
    while turn<4 {
        let discarded=utf8Encode("discarded");
        if flag { kept=bytesConcat(kept,utf8Encode("x")); }
        turn=turn+1;
    }
    return kept;
}
fn overwritten(seed: String) -> ByteArray {
    var value=utf8Encode(seed);
    let noise=utf8Encode("noise");
    value=utf8Encode("new");
    return value;
}
fn main() {}
"#,
    );
    let looped = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "looped")
        .unwrap();
    let plan = dead_local_roots(looped);
    assert!(!plan.is_empty());
    assert!(plan.iter().any(|(_, _, name)| name == "discarded"));
    let mut backedges = 0;
    for block in &looped.blocks {
        for (index, op) in block.operations.iter().enumerate() {
            if let OperationKind::GcSafepoint {
                reason: PollReason::LoopBackedge,
                stack_map,
            } = &op.kind
            {
                assert!(
                    stack_map
                        .roots
                        .iter()
                        .any(|r| matches!(r,StackRoot::Local{name,..} if name=="kept"))
                );
                assert!(!plan.contains(&(block.id, index, "kept".to_owned())));
                backedges += 1;
            }
        }
    }
    assert!(backedges > 0);
    let overwritten = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "overwritten")
        .unwrap();
    let plan = dead_local_roots(overwritten);
    assert!(plan.iter().any(|(_, _, name)| name == "value"));
    assert!(plan.iter().any(|(_, _, name)| name == "noise"));
}
#[test]
fn exception_before_overwrite_keeps_the_preexisting_local_for_its_handler() {
    let artifact = source(
        r#"
error E {}
fn risky(flag:Bool) { if flag { throw E(message:"expected"); } }
fn guarded(seed:ByteArray,flag:Bool)->ByteArray {
    var original=seed;
    let discarded=utf8Encode("discarded");
    try { risky(flag); original=utf8Encode("new"); }
    catch errors { return original; }
    return original;
}
fn main() {}
"#,
    );
    let f = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "guarded")
        .unwrap();
    let plan = dead_local_roots(f);
    assert!(
        !plan.is_empty(),
        "test must exercise optimized Error edges rather than a fallback"
    );
    let mut checked = 0;
    for block in &f.blocks {
        for (end, op) in block.operations.iter().enumerate() {
            if let OperationKind::ManagedCall { target, .. } = &op.kind
                && target == "risky"
            {
                let (index, point) = block.operations[..end]
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, p)| matches!(p.kind, OperationKind::GcSafepoint { .. }))
                    .unwrap();
                if let OperationKind::GcSafepoint { stack_map, .. } = &point.kind {
                    assert!(
                        stack_map
                            .roots
                            .iter()
                            .any(|r| matches!(r,StackRoot::Local{name,..} if name=="original"))
                    );
                    assert!(!plan.contains(&(block.id, index, "original".to_owned())));
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0);
}
#[test]
fn cleanup_and_async_native_indirect_child_bodies_keep_all_locals() {
    let artifact = source(
        r#"
native { fn tick()->Int symbol "tick"; }
fn nativeBody()->String {let held="held";let value=tick();return held;}
fn indirect(action:fn()->Int)->String {let held="held";let value=action();return held;}
async fn child()->String{return "child";}
fn creates()->Int{let held="held";let pending=child();return 1;}
fn cleanup()->String{let held="held";try{return held;}finally{let data=utf8Encode(held);}}
fn main(){}
"#,
    );
    for name in ["nativeBody", "indirect", "child", "creates", "cleanup"] {
        let f = artifact
            .ir
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap();
        assert!(dead_local_roots(f).is_empty(), "{name}");
    }
}
