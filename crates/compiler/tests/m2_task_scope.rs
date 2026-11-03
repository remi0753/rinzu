//! Regression coverage for the former task-group surface, now lexical try all.
use rinzu_compiler::{
    FileId, OperationKind, SourceFile, compile_source, format_source, verify_module,
};

const SOURCE: &str = r#"error E{} fn main(){try all{throw E(message:"e");return;}catch errors{}finally{let done=true;}}"#;

#[test]
fn lexical_collection_registers_cleanup_and_verifies_finish_outcome_and_roots() {
    let source = SourceFile::new(FileId(1), "scope.rinzu", SOURCE);
    let artifact = compile_source(&source).unwrap();
    let text = format_source(&source).unwrap();
    assert!(text.contains("try all {"));
    compile_source(&SourceFile::new(FileId(1), "scope.rinzu", text)).unwrap();
    assert!(
        !artifact
            .ir
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.operations)
            .any(|operation| matches!(
                operation.kind,
                OperationKind::JoinTaskScope { .. } | OperationKind::EnterTaskScope { .. }
            ))
    );
    for mutation in 0..6 {
        let mut ir = artifact.ir.clone();
        let function = ir
            .functions
            .iter_mut()
            .find(|function| function.name == "main")
            .unwrap();
        if mutation >= 4 {
            for block in &mut function.blocks {
                block.operations.retain(|operation| {
                    if mutation == 4 {
                        !matches!(operation.kind, OperationKind::ErrorGroupOutcome { .. })
                    } else {
                        !matches!(operation.kind, OperationKind::EnterErrorGroup { .. })
                    }
                });
            }
        } else {
            let block = function
                .blocks
                .iter_mut()
                .find(|block| {
                    block.operations.iter().any(|operation| {
                        matches!(operation.kind, OperationKind::FinishErrorGroup { .. })
                    })
                })
                .unwrap();
            let index = block
                .operations
                .iter()
                .position(|operation| {
                    matches!(operation.kind, OperationKind::FinishErrorGroup { .. })
                })
                .unwrap();
            match mutation {
                0 => {
                    block.operations.remove(index);
                }
                1 => {
                    if let OperationKind::FinishErrorGroup { scope, .. } =
                        &mut block.operations[index].kind
                    {
                        scope.start_byte += 1;
                    }
                }
                2 => {
                    block.operations.remove(index - 1);
                }
                _ => {
                    block
                        .operations
                        .insert(index, block.operations[index].clone());
                }
            }
        }
        assert!(verify_module(&ir).is_err(), "accepted mutation {mutation}");
    }
}

#[test]
fn async_cleanup_await_does_not_depend_on_try_all() {
    for text in [
        "fn main(){try all {} finally {}}",
        "fn main(){try{}finally{try all{}finally{}}}",
        "async fn child(){} async fn main(){try all{let pending=child();await pending;}finally{}}",
        "async fn child(){} async fn main(){try all{}finally{await child();}}",
        "async fn child(){} async fn main(){try{}finally{await child();}}",
        "async fn child(){} async fn main(){try all{}finally{try{}finally{await child();}}}",
        "async fn child(){} async fn main(){try all{}finally{try{}catch errors{}finally{await child();}}}",
    ] {
        compile_source(&SourceFile::new(FileId(2), "valid.rinzu", text)).unwrap();
    }
    for text in [
        "async fn child(){} fn main(){try{}finally{await child();}}",
        "async fn child(){} fn main(){try all{}finally{await child();}}",
    ] {
        let errors =
            compile_source(&SourceFile::new(FileId(2), "invalid.rinzu", text)).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == rinzu_compiler::DiagnosticCode::InvalidFinally),
            "{errors:?}"
        );
    }
}
