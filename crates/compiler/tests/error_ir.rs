use rinzu_compiler::{
    DiagnosticCode, FileId, OperationKind as O, SourceFile, Type, compile_source, verify_module,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(174), "error-ir.rinzu", text)
}

#[test]
fn error_cast_and_payload_projection_cannot_forge_an_unrelated_nominal_type() {
    let artifact = compile_source(&source(
        r#"
        error E { payload: Int }
        class Fake { message: String }
        fn accept(problem: Error) -> String { return problem.message; }
        fn main() { let message = accept(E(message: "x", payload: 7)); }
    "#,
    ))
    .unwrap();
    let mut forged = artifact.ir.clone();
    let cast = forged
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
        .flat_map(|block| &mut block.operations)
        .find(|operation| matches!(operation.kind, O::ErrorCast { .. }))
        .unwrap();
    cast.result.as_mut().unwrap().ty = Type::Named("Fake".into());
    assert!(verify_module(&forged).is_err());
    let mut forged = artifact.ir;
    let create = forged
        .functions
        .iter_mut()
        .flat_map(|function| &mut function.blocks)
        .flat_map(|block| &mut block.operations)
        .find(|operation| matches!(&operation.kind, O::Construct { target, .. } if target == "E"))
        .unwrap();
    let O::Construct { arguments, .. } = &mut create.kind else {
        unreachable!()
    };
    arguments.swap(1, 4);
    assert!(verify_module(&forged).is_err());
}

#[test]
fn only_single_errors_can_be_thrown_and_catch_types_and_bindings_are_checked() {
    for text in [
        "fn main() {throw 7;}",
        "error E {} fn main() {throw [E(message: \"x\")];}",
        "class Fake {message:String} fn main(){throw Fake(message: \"x\");}",
        "fn main(){try {} catch [value: Int] {} catch errors {}}",
        "error E {} fn main(){let value=1;try {} catch [value:E] {} catch errors {}}",
    ] {
        let diagnostics = compile_source(&source(text)).unwrap_err();
        assert!(
            diagnostics.iter().any(|diagnostic| matches!(
                diagnostic.code,
                DiagnosticCode::TypeMismatch | DiagnosticCode::DuplicateName
            )),
            "{text}: {diagnostics:?}"
        );
    }
    let diagnostics =
        compile_source(&source("fn main(){try {} catch first {} catch second {}}")).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::InvalidCatchPattern)
    );
}

#[test]
fn worker_root_failures_are_checked_even_when_the_caller_catches_the_future() {
    let diagnostics = compile_source(&source(
        r#"
        error E {}
        fn work() { throw E(message: "x"); }
        async fn main() { let pending = spawn work(); try {await pending;} catch errors {} }
    "#,
    ))
    .unwrap_err();
    assert!(diagnostics.iter().any(|diagnostic| diagnostic.code
        == DiagnosticCode::ErrorEscapesBoundary
        && diagnostic.message.contains("Worker root")));
}
