use rinzu_compiler::{
    DataOperation, FileId, OperationKind, SourceFile, StackRoot, compile_source,
    dead_temporary_roots,
};

#[test]
fn nested_operands_survive_allocation_and_dead_values_keep_their_local_roots() {
    let artifact = compile_source(&SourceFile::new(
        FileId(95),
        "root-liveness.rinzu",
        r#"
fn pair(text: String) -> ByteArray {
    return bytesConcat(utf8Encode(text), utf8Encode("right"));
}
fn dead(text: String) -> Int {
    let discarded = utf8Encode(text);
    let later = utf8Encode("later");
    return bytesLength(later);
}
fn main() {}
"#,
    ))
    .unwrap();
    let pair = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "pair")
        .unwrap();
    let plan = dead_temporary_roots(pair);
    let mut checked = 0;
    for block in &pair.blocks {
        for (end, op) in block.operations.iter().enumerate() {
            if let OperationKind::DataOperation {
                kind: DataOperation::BytesConcat,
                arguments,
                ..
            } = &op.kind
            {
                let first = arguments[0];
                let start = block
                    .operations
                    .iter()
                    .position(|op| op.result.as_ref().is_some_and(|v| v.id == first))
                    .unwrap();
                for (index, point) in block
                    .operations
                    .iter()
                    .enumerate()
                    .take(end)
                    .skip(start + 1)
                {
                    if let OperationKind::GcSafepoint { stack_map, .. } = &point.kind {
                        assert!(stack_map.roots.iter().any(
                            |root| matches!(root, StackRoot::Value {value,..} if *value==first)
                        ));
                        assert!(!plan.contains(&(block.id, index, first)));
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(
        checked > 0,
        "first operand must cross the second allocation"
    );
    let dead = artifact
        .ir
        .functions
        .iter()
        .find(|f| f.name == "dead")
        .unwrap();
    let plan = dead_temporary_roots(dead);
    assert!(!plan.is_empty());
    let mut kept_local = 0;
    for block in &dead.blocks {
        for (index, op) in block.operations.iter().enumerate() {
            if let OperationKind::GcSafepoint { stack_map, .. } = &op.kind
                && plan.iter().any(|(b, i, _)| *b == block.id && *i == index)
                && stack_map
                    .roots
                    .iter()
                    .any(|root| matches!(root,StackRoot::Local {name,..} if name=="discarded"))
            {
                kept_local += 1;
            }
        }
    }
    assert!(kept_local > 0);
}

#[test]
fn async_native_indirect_and_child_creation_bodies_keep_conservative_maps() {
    let artifact=compile_source(&SourceFile::new(FileId(96),"root-boundaries.rinzu",r#"
native { fn tick() -> Int symbol "tick"; }
fn nativeBody() -> ByteArray { let before=utf8Encode("before"); let value=tick(); return before; }
fn indirect(action: fn () -> Int) -> ByteArray { let before=utf8Encode("before"); let value=action(); return before; }
async fn child() -> String { return "child"; }
fn creates() -> Int { let pending=child(); return 1; }
fn main() {}
"#)).unwrap();
    for name in ["nativeBody", "indirect", "child", "creates"] {
        let f = artifact
            .ir
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap();
        assert!(dead_temporary_roots(f).is_empty(), "{name}");
    }
}
