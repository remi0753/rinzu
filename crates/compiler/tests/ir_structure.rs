use rinzu_compiler::{
    ConstantValue, DiagnosticCode, FileId, IrModule, OperationKind, SourceFile, Terminator, Type,
    compile_source, verify_module,
};

fn module() -> IrModule {
    compile_source(&SourceFile::new(
        FileId(1),
        "structure.rinzu",
        "fn answer(value: Int) -> Int { return value; } fn main() { if true {} else {} }",
    ))
    .unwrap()
    .ir
}

fn rejected(module: &IrModule, message: &str) {
    let errors = verify_module(module).expect_err("malformed IR must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::IrVerification
                && error.message.contains(message)),
        "{errors:#?}"
    );
}

#[test]
fn function_names_and_block_ids_are_unique() {
    let mut ir = module();
    ir.functions.push(ir.functions[0].clone());
    rejected(&ir, "function name");

    let mut ir = module();
    let function = &mut ir.functions[0];
    let block = function
        .blocks
        .iter()
        .find(|block| block.id == function.failure)
        .unwrap()
        .clone();
    function.blocks.push(block);
    rejected(&ir, "block identity");
}

#[test]
fn parameter_indices_and_types_match_the_function_signature() {
    for mutation in 0..3 {
        let mut ir = module();
        let parameter = ir.functions[0]
            .blocks
            .iter_mut()
            .flat_map(|block| &mut block.operations)
            .find(|operation| matches!(operation.kind, OperationKind::Parameter { .. }))
            .unwrap();
        match mutation {
            0 => parameter.kind = OperationKind::Parameter { index: u32::MAX },
            1 => parameter.result.as_mut().unwrap().ty = Type::String,
            _ => parameter.result = None,
        }
        rejected(&ir, "parameter");
    }
}

#[test]
fn branch_conditions_must_be_boolean() {
    let mut ir = module();
    let block = ir.functions[1]
        .blocks
        .iter_mut()
        .find(|block| matches!(block.terminator, Terminator::Branch { .. }))
        .unwrap();
    let Terminator::Branch { condition, .. } = block.terminator else {
        unreachable!()
    };
    let value = block
        .operations
        .iter_mut()
        .find(|operation| {
            operation
                .result
                .as_ref()
                .is_some_and(|value| value.id == condition)
        })
        .unwrap();
    value.kind = OperationKind::Constant(ConstantValue::Int(1));
    value.result.as_mut().unwrap().ty = Type::Int;
    rejected(&ir, "branch condition");
}

#[test]
fn returns_match_the_declared_result_including_unit() {
    let mut ir = module();
    ir.functions[0].result_type = Type::String;
    rejected(&ir, "return type");

    let mut ir = module();
    for block in &mut ir.functions[0].blocks {
        if let Terminator::Return { value, .. } = &mut block.terminator {
            *value = None;
        }
    }
    rejected(&ir, "return type");

    let mut ir = compile_source(&SourceFile::new(
        FileId(1),
        "unit.rinzu",
        "fn main() { let value = 1; }",
    ))
    .unwrap()
    .ir;
    let block = ir.functions[0]
        .blocks
        .iter_mut()
        .find(|block| matches!(block.terminator, Terminator::Return { .. }))
        .unwrap();
    let integer = block
        .operations
        .iter()
        .find_map(|operation| {
            matches!(
                operation.kind,
                OperationKind::Constant(ConstantValue::Int(_))
            )
            .then(|| operation.result.as_ref().unwrap().id)
        })
        .unwrap();
    let Terminator::Return { value, .. } = &mut block.terminator else {
        unreachable!()
    };
    *value = Some(integer);
    rejected(&ir, "return type");
}

#[test]
fn complete_returns_cleanup_and_divergent_expressions_still_verify() {
    let ir = compile_source(&SourceFile::new(
        FileId(1),
        "returns.rinzu",
        r#"
        error Stop {}
        fn choose(flag: Bool) -> Int { if flag { return 1; } else { return 2; } }
        fn cleanup(flag: Bool) -> Int { try { return choose(flag); } finally { let marker = 0; } }
        fn caught() -> Int { try { throw Stop(message: "stop"); } catch errors { return 3; } }
        fn diverge() -> Never { throw Stop(message: "stop"); }
        fn unreachableReturn() -> Int { return diverge(); }
        fn main() { let a = cleanup(true); let b = caught(); }
    "#,
    ))
    .unwrap()
    .ir;
    verify_module(&ir).unwrap();
}
