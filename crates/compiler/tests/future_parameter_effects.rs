use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

const BASE: &str = r#"
error A{} error B{} error C{}
async fn a(){throw A(message:"a");}
async fn ok(){}
"#;
fn accepted(text: &str) {
    compile_source(&SourceFile::new(
        FileId(243),
        "future-parameters.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap();
}
fn incomplete(text: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(243),
        "future-parameters.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::NonExhaustiveCatch),
        "{diagnostics:#?}"
    );
}
const CORRELATED: &str = r#"
async fn pair(flag:Bool){
if flag{try{throw A(message:"a");}finally{throw B(message:"b");}}
else{try{throw B(message:"b");}finally{throw A(message:"a");}}
}
"#;

#[test]
fn awaited_parameters_are_instantiated_with_the_callers_actual_future() {
    accepted(
        r#"
async fn forward(value:Future<Unit>){await value;}
async fn main(){try{await forward(a());}catch[A]{}}
"#,
    );
    incomplete(
        r#"
async fn forward(value:Future<Unit>){await value;}
async fn main(){try{await forward(a());}catch[B]{}}
"#,
    );
}

#[test]
fn complete_failure_words_keep_type_order_correlations() {
    accepted(&format!(
        r#"{CORRELATED}
async fn forward(value:Future<Unit>){{await value;}}
async fn main(){{try{{await forward(pair(true));}}catch[A,B]{{}}catch[B,A]{{}}}}
"#
    ));
    incomplete(&format!(
        r#"{CORRELATED}
async fn forward(value:Future<Unit>){{await value;}}
async fn main(){{try{{await forward(pair(true));}}catch[A,B]{{}}}}
"#
    ));
}

#[test]
fn finally_uses_the_same_original_word_on_repeated_await() {
    accepted(&format!(
        r#"{CORRELATED}
async fn repeated(value:Future<Unit>){{try all{{await value;}}finally{{await value;}}}}
async fn main(){{try{{await repeated(pair(true));}}
catch[A,B]{{}}catch[B,A]{{}}catch[A,B,A,B]{{}}catch[B,A,B,A]{{}}
}}
"#
    ));
}

#[test]
fn independent_cleanup_errors_keep_their_position_and_infallible_inputs_remove_latent_errors() {
    accepted(
        r#"
async fn cleanup(value:Future<Unit>){try{await value;}finally{throw C(message:"cleanup");}}
async fn main(){try{await cleanup(ok());}catch[C]{}}
"#,
    );
    incomplete(
        r#"
async fn cleanup(value:Future<Unit>){try{await value;}finally{throw C(message:"cleanup");}}
async fn main(){try{await cleanup(a());}catch[C]{}}
"#,
    );
}

#[test]
fn a_handler_is_not_reachable_when_the_future_has_no_normal_error() {
    accepted(
        r#"
async fn replace(value:Future<Unit>){try{await value;}catch errors{throw B(message:"replacement");}}
async fn main(){try{await replace(ok());}catch[A]{}}
"#,
    );
    accepted(
        r#"
async fn replace(value:Future<Unit>){try{await value;}catch errors{throw B(message:"replacement");}}
async fn main(){try{await replace(a());}catch[B]{}}
"#,
    );
}

#[test]
fn throwing_one_caught_error_does_not_rethrow_the_original_list() {
    accepted(&format!(
        r#"{CORRELATED}
async fn first(value:Future<Unit>){{try{{await value;}}catch[error:Error,...rest]{{throw error;}}}}
async fn main(){{try{{await first(pair(true));}}catch[A]{{}}catch[B]{{}}}}
"#
    ));
}

#[test]
fn await_success_values_preserve_error_types_and_record_projections() {
    for (make, receive) in [
        (
            "async fn make()->Error{return A(message:\"value\");}",
            "async fn receive(value:Future<Error>){let problem=await value;throw problem;}",
        ),
        (
            "async fn make()->{problem:Error}{return {problem=A(message:\"value\")};}",
            "async fn receive(value:Future<{problem:Error}>){let row=await value;throw row.problem;}",
        ),
    ] {
        accepted(&format!(
            "{make}{receive} async fn main(){{try{{await receive(make());}}catch[A]{{}}}}"
        ));
        incomplete(&format!(
            "{make}{receive} async fn main(){{try{{await receive(make());}}catch[B]{{}}}}"
        ));
    }
}

#[test]
fn future_parameters_in_const_fields_and_collections_preserve_their_latent_failure() {
    accepted(
        r#"
class Pending{const future:Future<Unit>}
async fn receive(value:Pending){await value.future;}
async fn main(){try{await receive(Pending(future:a()));}catch[A]{}}
"#,
    );
    accepted(
        r#"
async fn receive(values:List<Future<Unit>>){switch listGet(values,0){case Option.Some(future):{await future;}case Option.None:{}}}
async fn main(){let pending=a();try{await receive([pending]);}catch[A]{}try{await pending;}catch[A]{}}
"#,
    );
}

#[test]
fn recursion_and_direct_function_references_keep_future_inputs() {
    accepted(
        r#"
async fn forward(count:Int,value:Future<Unit>){if count==0{await value;return;}await forward(count-1,value);}
async fn main(){let action=forward;try{await action(3,a());}catch[A]{}}
"#,
    );
}

#[test]
fn async_closures_can_await_a_future_argument_with_an_ordinary_capture() {
    accepted(
        r#"
async fn main(){let count=3;let action=async fn(value:Future<Unit>){let marker=count;await value;};try{await action(a());}catch[A]{}}
"#,
    );
}

#[test]
fn separate_symbolic_prefixes_do_not_inherit_unrelated_future_guards() {
    let helper = r#"
async fn failing(){throw B(message:"b");}
fn raise(problem:Error){throw problem;}
async fn route(first:Future<Unit>,second:Future<Unit>){
var problem:Error=A(message:"default");
try{await first;}catch[one:Error,...restOne]{problem=one;}
try{await second;}catch[two:Error,...restTwo]{problem=two;}
raise(problem);
}
"#;
    accepted(&format!(
        "{helper} async fn main(){{try{{await route(ok(),failing());}}catch[A]{{}}catch[B]{{}}}}"
    ));
    incomplete(&format!(
        "{helper} async fn main(){{try{{await route(ok(),failing());}}catch[A]{{}}}}"
    ));
}
