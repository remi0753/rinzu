use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

#[test]
fn many_checked_closures_preserve_failure_handlers() {
    let mut source = String::from(
        r#"
import rinzu.list;
fn require(value:Bool){if !value{panic("invalid");}}
fn checked<T>(action:fn()->T,code:Int){
    try{let unexpected=action();require(false);}
    catch[problem:Error,...rest]{require(errorCode(problem)==code);require(rinzu.list.length(rest)==0);}
}
fn main(){let high=9223372036854775807;
"#,
    );
    for _ in 0..156 {
        source.push_str("checked<Int>(fn()->Int{return checkedAdd(high,1);},300);\n");
    }
    source.push('}');
    compile_source(&SourceFile::new(
        FileId(401),
        "many-checked-closures.rinzu",
        source,
    ))
    .unwrap();
}

#[test]
fn repeated_guard_merges_do_not_hide_a_different_error_type() {
    let mut source = String::from(
        "error A{} error B{} fn observe(action:fn()->Unit){try{action();}catch[A]{}}fn main(){",
    );
    for _ in 0..64 {
        source.push_str("observe(fn(){throw A(message:\"a\");});");
    }
    source.push_str("observe(fn(){throw B(message:\"b\");});}");
    let errors =
        compile_source(&SourceFile::new(FileId(402), "many-guards.rinzu", source)).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::NonExhaustiveCatch),
        "{errors:#?}"
    );
}

fn long_predicate(operator: &str) -> String {
    fn balanced(values: &[String], operator: &str) -> String {
        if values.len() == 1 {
            return values[0].clone();
        }
        let middle = values.len() / 2;
        format!(
            "({} {operator} {})",
            balanced(&values[..middle], operator),
            balanced(&values[middle..], operator)
        )
    }
    let parts = balanced(
        &(0..40)
            .map(|index| format!("value=={index}"))
            .collect::<Vec<_>>(),
        operator,
    );
    format!("fn predicate(value:Int)->Bool{{return {parts};}}")
}
#[test]
fn long_short_circuit_expressions_have_finite_observation_states() {
    for operator in ["||", "&&"] {
        let source = format!(
            "{} fn main(){{let result=predicate(19);}}",
            long_predicate(operator)
        );
        compile_source(&SourceFile::new(
            FileId(403),
            "long-short-circuit.rinzu",
            source,
        ))
        .unwrap();
    }
}
#[test]
fn coalesced_expression_states_preserve_future_observation_and_early_returns() {
    for operator in ["||", "&&"] {
        let common = format!(
            "{} error Failed{{}} async fn work(){{throw Failed(message:\"failed\");}}",
            long_predicate(operator)
        );
        let source = format!(
            "{common} async fn main(){{let pending=work();let flag=predicate(19);try{{await pending;}}catch errors{{}}}}"
        );
        compile_source(&SourceFile::new(FileId(404), "long-observed.rinzu", source)).unwrap();
        let source = format!(
            "{common} async fn main(){{let pending=work();if predicate(19){{return;}}try{{await pending;}}catch errors{{}}}}"
        );
        let errors = compile_source(&SourceFile::new(
            FileId(405),
            "long-lost-observer.rinzu",
            source,
        ))
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::FutureDiscarded),
            "{errors:#?}"
        );
    }
}

#[test]
fn forwarded_failure_effects_are_independent_of_declaration_order() {
    for reverse in [false, true] {
        for failed in ["A", "B"] {
            let mut declarations = (0..64)
                .map(|index| {
                    format!(
                        "fn forward{index}()->Future<Unit>{{return forward{}();}}",
                        index + 1
                    )
                })
                .collect::<Vec<_>>();
            declarations.push(format!(
                "async fn forward64(){{throw {failed}(message:\"failure\");}}"
            ));
            if reverse {
                declarations.reverse();
            }
            let source = format!(
                "error A{{}}error B{{}}{}async fn main(){{try{{await forward0();}}catch[A]{{}}}}",
                declarations.join("\n")
            );
            let result = compile_source(&SourceFile::new(
                FileId(407),
                "forwarded-effects.rinzu",
                source,
            ));
            if failed == "A" {
                result.unwrap();
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .iter()
                        .any(|error| error.code == DiagnosticCode::NonExhaustiveCatch)
                );
            }
        }
    }
}

#[test]
fn short_circuit_results_preserve_conditional_future_observation() {
    for operator in ["||", "&&"] {
        let common = format!(
            "{} error Failed{{}} async fn fail(){{throw Failed(message:\"failed\");}} \
             async fn observe(value:Future<Unit>)->Bool{{try{{await value;}}catch errors{{}}return false;}}",
            long_predicate(operator)
        );
        let body = format!(
            "let pending=fail();let result=predicate(19) {operator} await observe(pending);"
        );
        compile_source(&SourceFile::new(
            FileId(408),
            "conditional-observer.rinzu",
            format!("{common} async fn main(){{{body}try{{await pending;}}catch errors{{}}}}"),
        ))
        .unwrap();
        let errors = compile_source(&SourceFile::new(
            FileId(409),
            "skipped-observer.rinzu",
            format!("{common} async fn main(){{{body}}}"),
        ))
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.code == DiagnosticCode::FutureDiscarded)
        );
    }
}

#[test]
fn multiple_native_record_snapshots_keep_finite_short_circuit_comparisons() {
    let mut source = String::from(
        "native {record Packet {a:Int,b:CBool,c:CDouble}fn load()->Packet symbol \"load\";} \
         type Snapshot={a:Int,b:Bool,c:Float}; \
         fn sample()->Snapshot {try{let packet=load();return {a=packet.a,b=packet.b,c=packet.c};}catch errors{return {a=0,b=false,c=0.0};}} \
         async fn main(){let original=sample();",
    );
    for index in 0..8 {
        source.push_str(&format!("let f{index}=spawn sample();"));
    }
    for index in 0..8 {
        source.push_str(&format!("let r{index}=await f{index};"));
    }
    source.push_str("if ");
    source.push_str(
        &(0..8)
            .map(|index| format!("original!=r{index}"))
            .collect::<Vec<_>>()
            .join(" || "),
    );
    source.push_str(" {panic(\"invalid\");}}");
    compile_source(&SourceFile::new(
        FileId(410),
        "native-record-futures.rinzu",
        source,
    ))
    .unwrap();
}
