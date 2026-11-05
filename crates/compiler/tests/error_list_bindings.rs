use rinzu_compiler::{FileId, OperationKind, SourceFile, Type, compile_source};

fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(178), "error-list.rinzu", text))
}

#[test]
fn every_whole_catch_binding_has_the_same_list_type_and_ir() {
    let artifact = compile(
        r#"
        error E{}
        fn retain(errors:List<Error>)->List<Error>{return errors;}
        async fn main(){
            try{throw E(message:"single");}catch errors{let saved:List<Error> = retain(errors);}
            try{throw E(message:"typed");}catch[E]{}catch errors{let saved:List<Error> = errors;}
            try all{let bad=CInt8(300);}catch errors{let saved:List<Error> = errors;}
        }
    "#,
    )
    .unwrap();
    assert!(
        !artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|operation| matches!(operation.kind, OperationKind::CatchError { .. }))
    );
    assert!(
        artifact
            .checked
            .expression_types
            .values()
            .any(|ty| *ty == Type::List(Box::new(Type::Error)))
    );
}

#[test]
fn whole_list_is_not_a_scalar_error_and_cannot_be_thrown() {
    for body in [
        "let wrong=errors.message;",
        "throw errors;",
        "let wrong=errorCode(errors);",
        "errors=[];",
    ] {
        assert!(
            compile(&format!(
                "error E{{}} fn main(){{try{{throw E(message:\"e\");}}catch errors{{{body}}}}}"
            ))
            .is_err(),
            "{body}"
        );
    }
}

#[test]
fn lists_can_be_retained_returned_and_captured_in_the_same_domain() {
    compile(r#"import rinzu.List;

        error A{} error B{}
        fn collect()->List<Error>{try{try{throw A(message:"a");}finally{throw B(message:"b");}}catch errors{return errors;}return [];}
        fn main(){let values=collect();let get=fn()->List<Error>{return values;};let count=rinzu.List.length(get());}
    "#).unwrap();
}
