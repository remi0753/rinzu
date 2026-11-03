use rinzu_compiler::{DiagnosticCode, FileId, OperationKind, SourceFile, compile_source};
fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(180), "error-all.rinzu", text))
}
fn rejected(text: &str) {
    let errors = compile(text).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::NonExhaustiveCatch),
        "{errors:?}"
    );
}

#[test]
fn statement_collection_is_synchronous_and_has_no_task_group_or_implicit_join() {
    let artifact = compile(
        r#"
        error A{} error B{}
        fn work(){try all{throw A(message:"a");throw B(message:"b");}catch[A,B]{}finally{}}
        fn main(){work();}
    "#,
    )
    .unwrap();
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .all(|function| function.facts.worker_safe && !function.facts.may_await)
    );
    let operations = artifact
        .ir
        .functions
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.operations)
        .collect::<Vec<_>>();
    assert!(
        operations
            .iter()
            .any(|operation| matches!(operation.kind, OperationKind::EnterErrorGroup { .. }))
    );
    assert!(operations.iter().any(|operation| matches!(
        operation.kind,
        OperationKind::FinishErrorGroup { handles: true, .. }
    )));
    assert!(!operations.iter().any(|operation| matches!(
        operation.kind,
        OperationKind::EnterTaskScope { .. } | OperationKind::JoinTaskScope { .. }
    )));
}

#[test]
fn exact_all_patterns_keep_statement_order_and_exhaustiveness() {
    for handlers in ["catch[A]{}", "catch[B,A]{}", "catch[A]{}catch[B]{}"] {
        rejected(&format!(
            "error A{{}} error B{{}} fn main(){{try all{{throw A(message:\"a\");throw B(message:\"b\");}}{handlers}}}"
        ));
    }
    compile("error A{} error B{} fn main(){try all{throw A(message:\"a\");throw B(message:\"b\");}catch[A,...rest]{} }").unwrap();
}

#[test]
fn repeating_collection_requires_an_arbitrary_length_pattern_and_nested_groups_are_local() {
    compile(r#"
        error A{}
        fn work(again:Bool){try all{while again{throw A(message:"a");}}catch[A,...rest]{} }
        fn main(){try all{try all{throw A(message:"inner");}catch[A]{}throw A(message:"outer");}catch[A]{} }
    "#).unwrap();
    rejected(
        "error A{} fn work(again:Bool){try all{while again{throw A(message:\"a\");}}catch[A]{} }fn main(){}",
    );
}

#[test]
fn a_failed_initializer_exposes_the_defined_unbound_read_error() {
    compile(
        r#"
        error A{}
        fn fail()->Int{throw A(message:"a");}
        fn main(){try all{let unavailable=fail();let copy=unavailable;}catch[A,Error]{} }
    "#,
    )
    .unwrap();
    rejected(
        r#"
        error A{}
        fn fail()->Int{throw A(message:"a");}
        fn main(){try all{let unavailable=fail();let copy=unavailable;}catch[A]{} }
    "#,
    );
}

#[test]
fn repeated_await_does_not_allow_an_unsound_doubleton_only_catch() {
    rejected(
        r#"
        error A{}
        async fn fail(){throw A(message:"a");}
        async fn main(){let pending=fail();try all{await pending;await pending;}catch[A,A]{} }
    "#,
    );
}

#[test]
fn all_without_a_catch_propagates_and_can_run_inside_synchronous_finally() {
    compile(
        r#"
        error A{} error B{}
        fn work(){try all{throw A(message:"a");}finally{throw B(message:"b");}}
        fn main(){try{work();}catch[A,B]{}finally{try all{throw A(message:"cleanup");}catch[A]{} }}
    "#,
    )
    .unwrap();
}
