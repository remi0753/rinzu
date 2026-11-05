use rinzu_compiler::{
    CompilerOptions, DiagnosticCode, FileId, NativeCallClass, NativeFunction, SourceFile, Type,
    compile_source, compile_source_with_options,
};

fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(
        FileId(248),
        "normal-error-origins.rinzu",
        text,
    ))
}
fn incomplete(text: &str) {
    let diagnostics = compile(text).unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::NonExhaustiveCatch),
        "{diagnostics:#?}"
    );
}

#[test]
fn an_unbound_function_target_is_a_normal_error_before_argument_evaluation() {
    let body = r#"
error A{} error B{}
fn ready(value:Int){}
fn make()->fn(Int)->Unit{throw A(message:"initializer");}
fn argument()->Int{throw B(message:"must not run");}
fn run(){try all{let action=make();action(argument());}CATCH}
fn main(){run();}
"#;
    compile(&body.replace("CATCH", "catch[A,Error]{}")).unwrap();
    incomplete(&body.replace("CATCH", "catch[A]{}"));
    incomplete(&body.replace("CATCH", "catch[A,B]{}"));
}

#[test]
fn an_unbound_class_receiver_is_checked_before_dispatch() {
    compile(
        r#"
error A{}
class Handler{const action:fn()->Unit}
fn ready(){}
fn make()->Handler{throw A(message:"initializer");}
fn main(){try all{let handler=make();handler.action();}catch[A,Error]{}}
"#,
    )
    .unwrap();
    incomplete(
        r#"
error A{}
class Handler{const action:fn()->Unit}
fn ready(){}
fn make()->Handler{throw A(message:"initializer");}
fn main(){try all{let handler=make();handler.action();}catch[A]{}}
"#,
    );
}

#[test]
fn synchronous_scalar_native_calls_and_explicit_callback_fallbacks_have_no_implicit_error() {
    compile(r#"
error A{} error B{}
native{fn scalar(value:Int)->Int symbol "scalar";fn empty() symbol "empty";fn invoke(action:fn()->Int)->Int symbol "invoke";}
fn action()->Int fallback -1 {throw B(message:"handled by adapter");}
fn main(){try{let value=scalar(7);empty();let result=invoke(action);}catch[A]{}}
"#).unwrap();
}

#[test]
fn native_result_reception_and_record_access_are_not_assumed_safe() {
    for declarations in [
        "fn receive()->String symbol \"receive\";",
        "fn receive()->Bytes(4) symbol \"receive\";",
        "opaque Handle; fn receive()->Handle symbol \"receive\";",
        "record Pair{value:Int} fn receive()->Pair symbol \"receive\";",
    ] {
        let text = format!(
            "error A{{}} native{{{declarations}}} fn main(){{try{{let result=receive();}}catch[A]{{}}}}"
        );
        incomplete(&text);
        compile(&text.replace("catch[A]{}", "catch[Error]{}")).unwrap();
    }
    for operation in ["fill(output);", "let value=output.value;"] {
        let text = format!(
            r#"error A{{}} native{{record Pair{{value:Int}} fn fill(value:Pair) symbol "fill";}}
fn main(){{let output=Pair(value:1);try{{{operation}}}catch[A]{{}}}}"#
        );
        let compile_sdk = |text: &str| {
            let mut options = CompilerOptions::default();
            options.sdk_files.insert(FileId(248));
            compile_source_with_options(
                &SourceFile::new(FileId(248), "storage-origins.rinzu", text),
                &options,
            )
        };
        let diagnostics = compile_sdk(&text).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::NonExhaustiveCatch),
            "{diagnostics:#?}"
        );
        compile_sdk(&text.replace("catch[A]{}", "catch[Error]{}")).unwrap();
    }
}

#[test]
fn numeric_conversions_are_errors_but_identity_conversions_and_arithmetic_traps_are_not() {
    for (input, output) in [
        ("Int", "CInt8"),
        ("Float", "Int"),
        ("Int", "CUInt64"),
        ("Float", "CFloat"),
    ] {
        let text = format!(
            "error A{{}} fn run(value:{input}){{try{{let result={output}(value);}}catch[A]{{}}}} fn main(){{}}"
        );
        incomplete(&text);
        compile(&text.replace("catch[A]{}", "catch[Error]{}")).unwrap();
    }
    compile("error A{} fn run(value:Int){try{let identity=Int(value);let trap=value/0;}catch[A]{}} fn main(){}").unwrap();
}

#[test]
fn control_validation_and_normal_absence_do_not_throw_normal_errors() {
    compile(
        r#"import rinzu.List;
error A{} async fn main(){try{
        let invalid=setProcessExitStatus(300);let absent=rinzu.List.get([1],-1);
    }catch[A]{}}"#,
    )
    .unwrap();
}

#[test]
fn checked_primitive_failures_are_normal_error_effects() {
    for call in [
        "parseInt(\"not a number\")",
        "shiftLeft(1,-1)",
        "checkedAdd(9223372036854775807,1)",
    ] {
        let text = format!("error A{{}} async fn main(){{try{{let value={call};}}catch[A]{{}}}}");
        let errors = compile(&text).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::NonExhaustiveCatch),
            "{errors:?}"
        );
        compile(&format!(
            "async fn main(){{try{{let value={call};}}catch[problem:Error,...rest]{{}}}}"
        ))
        .unwrap();
    }
}

#[test]
fn timer_call_validation_and_awaited_failures_remain_separate_error_sources() {
    let declarations = "import rinzu.timer; error A{}";
    incomplete(&format!(
        "{declarations} async fn main(){{try{{let future=rinzu.timer.sleep(-1);}}catch[A]{{}}}}"
    ));
    incomplete(&format!(
        "{declarations} async fn main(){{let future=rinzu.timer.sleep(1);try{{await future;}}catch[A]{{}}}}"
    ));
    compile(&format!(
        "{declarations} async fn main(){{try{{await rinzu.timer.sleep(1);}}catch errors{{}}}}"
    ))
    .unwrap();
}

#[test]
fn scalar_constructor_precedence_and_local_callable_shadowing_match_checked_semantics() {
    for declaration in [
        "fn CInt8(value:Int)->CInt8{return 7;}",
        "native{fn CInt8(value:Int)->CInt8 symbol \"named_scalar\";}",
    ] {
        let direct = compile(&format!(
            "{declaration} fn main(){{try{{let value=CInt8(300);}}catch errors{{}}}}"
        ))
        .unwrap();
        let main = direct
            .ir
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert!(
            main.blocks
                .iter()
                .flat_map(|block| &block.operations)
                .any(|operation| matches!(
                    operation.kind,
                    rinzu_compiler::OperationKind::NumericConvert { .. }
                ))
        );
        let reference = compile(&format!(
            "{declaration} fn main(){{let CInt8=CInt8;let value=CInt8(300);}}"
        ))
        .unwrap();
        let main = reference
            .ir
            .functions
            .iter()
            .find(|function| function.name == "main")
            .unwrap();
        assert!(
            !main
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .any(|operation| matches!(
                    operation.kind,
                    rinzu_compiler::OperationKind::NumericConvert { .. }
                ))
        );
        assert!(
            main.blocks
                .iter()
                .flat_map(|block| &block.operations)
                .any(|operation| matches!(
                    operation.kind,
                    rinzu_compiler::OperationKind::IndirectCall { .. }
                ))
        );
        incomplete(&format!(
            "error A{{}} {declaration} fn main(){{try{{let value=CInt8(300);}}catch[A]{{}}}}"
        ));
    }
}

#[test]
fn an_external_binding_without_a_normal_error_contract_does_not_prove_a_singleton_or_empty_effect()
{
    let mut options = CompilerOptions::default();
    options.native_functions.insert(
        "rinzu.test.external".into(),
        NativeFunction {
            parameters: vec![],
            result: Type::Unit,
            call_class: NativeCallClass::NativeSafe,
            metadata_complete: true,
            cleanup_safe: true,
        },
    );
    for catch in ["catch[A]{}", "catch[Error]{}"] {
        let source = SourceFile::new(
            FileId(250),
            "external-error-contract.rinzu",
            format!(
                "import rinzu.test; error A{{}} fn main(){{try{{rinzu.test.external();}}{catch}}}"
            ),
        );
        let diagnostics = compile_source_with_options(&source, &options).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::NonExhaustiveCatch),
            "{diagnostics:#?}"
        );
    }
    compile_source_with_options(
        &SourceFile::new(
            FileId(250),
            "external-error-contract.rinzu",
            "import rinzu.test; fn main(){try{rinzu.test.external();}catch errors{}}",
        ),
        &options,
    )
    .unwrap();
}
