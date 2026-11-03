use rinzu_compiler::{
    DiagnosticCode, FileId, SourceFile, compile_source, format_source, parse_source,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(176), "error-is.rinzu", text)
}

#[test]
fn is_parses_generic_targets_with_comparison_precedence_and_formats_idempotently() {
    let input = source(
        "error E<T>{payload:T} fn probe(problem:Error)->Bool{return problem is E<Int> && problem.payload>0;} fn is(value:Int)->Int{return value;} fn main(){let value=is(1);}",
    );
    parse_source(&input).unwrap();
    let formatted = format_source(&input).unwrap();
    assert!(formatted.contains("is E<Int>"));
    assert_eq!(format_source(&source(&formatted)).unwrap(), formatted);
    compile_source(&source(&formatted)).unwrap();
}

#[test]
fn stable_common_error_bindings_narrow_in_true_negated_else_and_short_circuit_branches() {
    compile_source(&source(
        r#"
        error E<T> {payload:T}
        error F {}
        fn take(value:Int) {}
        fn inspect(problem:Error) {
            if problem is E<Int> {take(problem.payload);}
            if !(problem is E<Int>) {} else {take(problem.payload);}
            if problem is E<Int> && problem.payload>0 {take(problem.payload);}
            if !(problem is E<Int>) || problem.payload>0 {} else {take(problem.payload);}
            while problem is E<Int> {take(problem.payload);break;}
            if problem is F {} else if problem is E<Int> {take(problem.payload);}
        }
        fn main(){inspect(E<Int>(message:"x",payload:7));}
    "#,
    ))
    .unwrap();
}

#[test]
fn narrowing_does_not_escape_its_branch_or_apply_to_mutable_bindings_and_field_rereads() {
    for text in [
        "error E{payload:Int} fn read(e:Error){if e is E {} let wrong=e.payload;} fn main(){}",
        "error E{payload:Int} fn read(e:Error){if !(e is E) {let wrong=e.payload;}} fn main(){}",
        "error E{payload:Int} fn main(){var e:Error=E(message:\"x\",payload:7);if e is E {let wrong=e.payload;}}",
        "error E{payload:Int} class Holder{error:Error} fn read(holder:Holder){if holder.error is E {let wrong=holder.error.payload;}} fn main(){}",
    ] {
        let diagnostics = compile_source(&source(text)).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::UnknownName
                    && diagnostic.message.contains("payload")),
            "{text}: {diagnostics:?}"
        );
    }
}

#[test]
fn is_requires_an_error_value_and_a_concrete_error_target() {
    for text in [
        "error E{} fn main(){let bad=7 is E;}",
        "error E{} fn main(){let e=E(message:\"x\");let bad=e is Int;}",
        "error E{} class Fake{message:String} fn main(){let e=E(message:\"x\");let bad=e is Fake;}",
        "error E{} fn main(){let e=E(message:\"x\");let bad=e is Error;}",
    ] {
        let diagnostics = compile_source(&source(text)).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::TypeMismatch
                    && diagnostic.message.contains("is requires")),
            "{text}: {diagnostics:?}"
        );
    }
}

#[test]
fn generic_function_substitution_and_aliases_preserve_error_test_identity() {
    compile_source(&source(r#"
        type T=String;
        type Count=Int;
        error E<T>{payload:T}
        fn matches<T>(problem:Error)->Bool{return problem is E<T>;}
        fn main(){let yes=matches<Count>(E<Int>(message:"x",payload:7));let no=matches<String>(E<Int>(message:"x",payload:7));}
    "#)).unwrap();
    assert!(
        compile_source(&source("class SourceLocation{} fn main(){}"))
            .unwrap_err()
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::DuplicateName)
    );
}
