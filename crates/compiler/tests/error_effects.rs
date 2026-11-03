use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

fn compile(
    text: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(177), "error-effects.rinzu", text))
}

fn accepted(text: &str) {
    compile(text).unwrap();
}
fn rejected(text: &str, code: DiagnosticCode) {
    let errors = compile(text).unwrap_err();
    assert!(errors.iter().any(|error| error.code == code), "{errors:?}");
}

#[test]
fn singleton_catches_need_no_fallback_when_every_callee_error_is_covered() {
    accepted(
        r#"
        error A {} error B {}
        fn fail(choice:Bool) {if choice {throw A(message:"a");} else {throw B(message:"b");}}
        fn relay(choice:Bool) {fail(choice);}
        fn main(){try{relay(true);}catch[a:A]{}catch[b:B]{}}
    "#,
    );
    rejected(
        r#"
        error A {} error B {}
        fn fail(choice:Bool) {if choice {throw A(message:"a");} else {throw B(message:"b");}}
        fn main(){try{fail(true);}catch[a:A]{}}
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn finally_preserves_error_order_and_exact_lengths() {
    accepted(
        r#"
        error A{} error C{}
        fn fail(){try{throw A(message:"a");}finally{throw C(message:"c");}}
        fn main(){try{fail();}catch[a:A,c:C]{}}
    "#,
    );
    for pattern in [
        "catch[a:A]{}",
        "catch[c:C,a:A]{}",
        "catch[a:A]{}catch[c:C]{}",
    ] {
        rejected(
            &format!(
                r#"error A{{}} error C{{}}
            fn fail(){{try{{throw A(message:"a");}}finally{{throw C(message:"c");}}}}
            fn main(){{try{{fail();}}{pattern}}}"#
            ),
            DiagnosticCode::NonExhaustiveCatch,
        );
    }
    accepted(
        r#"
        error A{} error C{}
        fn fail(){try{throw A(message:"a");}finally{throw C(message:"c");}}
        fn main(){try{fail();}catch[a:A,...rest]{}}
    "#,
    );
}

#[test]
fn local_incomplete_catch_is_an_error_even_with_an_outer_fallback() {
    rejected(
        r#"
        error A{} error C{}
        fn main(){try{try{throw C(message:"c");}catch[a:A]{}}catch errors{}}
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
    accepted(
        r#"
        error A{} error C{}
        fn main(){try{try{throw C(message:"c");}catch[a:A]{}catch errors{}}catch outside{}}
    "#,
    );
}

#[test]
fn parameter_aliases_returned_errors_and_generic_identities_are_instantiated() {
    accepted(
        r#"
        error E<T>{payload:T}
        type Count=Int;
        fn relay(problem:Error){let alias=problem;throw alias;}
        fn identity(problem:Error)->Error{return problem;}
        fn make()->Error{return E<Int>(message:"x",payload:7);}
        fn main(){
            try{relay(identity(make()));}catch[e:E<Count>]{}
        }
    "#,
    );
    rejected(
        r#"
        error E<T>{payload:T}
        fn relay(problem:Error){throw problem;}
        fn main(){try{relay(E<Int>(message:"x",payload:7));}catch[e:E<String>]{} }
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn recursive_cleanup_words_keep_a_bounded_prefix_and_a_longer_length_category() {
    accepted(
        r#"
        error A{} error C{}
        fn fail(depth:Int){
            if depth==0{throw A(message:"a");}
            try{fail(depth-1);}finally{throw C(message:"c");}
        }
        fn main(){try{fail(10);}catch[a:A]{}catch[a:A,c:C,...rest]{}}
    "#,
    );
    rejected(
        r#"
        error A{} error C{}
        fn fail(depth:Int){if depth==0{throw A(message:"a");}try{fail(depth-1);}finally{throw C(message:"c");}}
        fn main(){try{fail(10);}catch[a:A]{}catch[a:A,c:C]{} }
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn mutual_recursion_and_function_references_contribute_all_possible_errors() {
    accepted(
        r#"
        error A{} error B{}
        fn one(depth:Int){if depth==0{throw A(message:"a");}two(depth-1);}
        fn two(depth:Int){if depth==0{throw B(message:"b");}one(depth-1);}
        fn main(){let chosen:fn(Int)->Unit=one;try{chosen(2);}catch[A]{}catch[B]{}}
    "#,
    );
    rejected(
        r#"
        error A{} error B{}
        fn one(depth:Int){if depth==0{throw A(message:"a");}two(depth-1);}
        fn two(depth:Int){if depth==0{throw B(message:"b");}one(depth-1);}
        fn main(){let chosen:fn(Int)->Unit=one;try{chosen(2);}catch[A]{}}
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn await_observes_failure_at_that_expression_including_repeated_observation() {
    accepted(
        r#"
        error A{}
        async fn work()->Int{throw A(message:"a");}
        async fn main(){
            try{let value=await work();}catch[A]{}
            let pending=work();
            try{let first=await pending;}catch[A]{}
            try{let again=await pending;}catch[A]{}
        }
    "#,
    );
    rejected(
        r#"
        error A{} error B{}
        async fn work()->Int{throw A(message:"a");}
        async fn main(){try{let value=await work();}catch[B]{} }
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn catches_do_not_catch_errors_from_their_own_handlers_or_cleanup() {
    accepted(
        r#"
        error A{} error B{} error C{}
        fn fail(){try{throw A(message:"a");}catch[A]{throw B(message:"b");}finally{throw C(message:"c");}}
        fn main(){try{fail();}catch[B,C]{}}
    "#,
    );
    rejected(
        r#"
        error A{} error B{}
        fn fail(){try{throw A(message:"a");}catch[A]{throw B(message:"b");}catch[B]{} }
        fn main(){try{fail();}catch[A]{} }
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn preceding_pattern_subsumption_includes_unions_of_different_lengths() {
    for handlers in [
        "catch[A]{}catch[A]{}catch errors{}",
        "catch[A,...rest]{}catch[A]{}catch errors{}",
        "catch[A]{}catch[A,Error,...rest]{}catch[A,...rest]{}catch errors{}",
    ] {
        rejected(
            &format!("error A{{}} fn main(){{try{{}}{handlers}}}"),
            DiagnosticCode::InvalidCatchPattern,
        );
    }
    accepted("error A{} error B{} fn main(){try{}catch[A]{}catch[B]{}catch errors{}}");
}

#[test]
fn open_common_errors_require_an_open_catch_but_not_a_whole_list_binding() {
    accepted("fn relay(problem:Error){try{throw problem;}catch[e:Error]{}} fn main(){}");
    rejected(
        "error A{} fn relay(problem:Error){try{throw problem;}catch[A]{}} fn main(){}",
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn arithmetic_faults_are_not_invented_as_normal_errors_and_native_io_is_not_assumed_safe() {
    accepted("error A{} fn main(){try{let value=1/0;}catch[A]{}}");
    rejected(
        "import rinzu.stdio; error A{} fn main(){try{rinzu.stdio.writeLine(7);}catch[A]{}}",
        DiagnosticCode::NonExhaustiveCatch,
    );
    accepted("import rinzu.stdio; fn main(){try{rinzu.stdio.writeLine(7);}catch[e:Error]{}}");
}

#[test]
fn awaiting_the_parent_does_not_observe_an_abandoned_child_failure() {
    rejected(
        r#"
        error A{}
        async fn child(){throw A(message:"a");}
        async fn parent(){let pending=child();}
        async fn main(){try{await parent();}catch[A]{} }
    "#,
        DiagnosticCode::FutureDiscarded,
    );
}

#[test]
fn unreachable_assignments_do_not_change_the_error_seen_by_finally() {
    accepted(
        r#"
        error A{} error B{}
        fn fail(){var saved:Error=A(message:"a");try{throw A(message:"first");saved=B(message:"dead");}finally{throw saved;}}
        fn main(){try{fail();}catch[A,A]{}}
    "#,
    );
    rejected(
        r#"
        error A{} error B{}
        fn fail(){var saved:Error=A(message:"a");try{throw A(message:"first");saved=B(message:"dead");}finally{throw saved;}}
        fn main(){try{fail();}catch[A,B]{}}
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
}

#[test]
fn cleanup_reads_include_states_at_earlier_failing_statements() {
    rejected(
        r#"
        error A{} error B{} error C{}
        fn sometimes(fail:Bool){if fail{throw C(message:"c");}}
        fn work(fail:Bool){var saved:Error=A(message:"a");try{sometimes(fail);saved=B(message:"b");}finally{throw saved;}}
        fn main(){try{work(true);}catch[C,B]{}catch[B]{} }
    "#,
        DiagnosticCode::NonExhaustiveCatch,
    );
    accepted(
        r#"
        error A{} error B{} error C{}
        fn sometimes(fail:Bool){if fail{throw C(message:"c");}}
        fn work(fail:Bool){var saved:Error=A(message:"a");try{sometimes(fail);saved=B(message:"b");}finally{throw saved;}}
        fn main(){try{work(true);}catch[C,Error]{}catch[e:Error]{} }
    "#,
    );
}
