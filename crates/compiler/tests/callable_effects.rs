use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

const BASE: &str = r#"
error A{} error B{}
async fn a(){throw A(message:"a");}
async fn b(){throw B(message:"b");}
"#;
fn accepted(text: &str) {
    compile_source(&SourceFile::new(
        FileId(245),
        "callable-effects.rinzu",
        format!("{BASE}{text}"),
    ))
    .unwrap();
}
fn incomplete(text: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(245),
        "callable-effects.rinzu",
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

#[test]
fn selected_async_closure_excludes_unused_functions_with_the_same_signature() {
    accepted(
        r#"async fn main(){let count=3;let action=async fn(){let marker=count;throw A(message:"selected");};try{await action();}catch[A]{}}"#,
    );
    incomplete(
        r#"async fn main(){let count=3;let action=async fn(){let marker=count;throw B(message:"selected");};try{await action();}catch[A]{}}"#,
    );
}

#[test]
fn captured_common_errors_keep_the_actual_error_identity() {
    accepted(
        r#"async fn main(){let problem:Error=A(message:"captured");let action=async fn(){throw problem;};try{await action();}catch[A]{}}"#,
    );
    incomplete(
        r#"async fn main(){let problem:Error=B(message:"captured");let action=async fn(){throw problem;};try{await action();}catch[A]{}}"#,
    );
}

#[test]
fn raw_function_reference_arguments_select_the_actual_function() {
    accepted(
        r#"
async fn invoke(action:fn()->Future<Unit>){await action();}
async fn main(){try{await invoke(a);}catch[A]{}}
"#,
    );
    incomplete(
        r#"
async fn invoke(action:fn()->Future<Unit>){await action();}
async fn main(){try{await invoke(b);}catch[A]{}}
"#,
    );
}

#[test]
fn immutable_function_fields_and_returned_closures_keep_their_environment() {
    accepted(
        r#"
class Handler{const action:fn()->Future<Unit>}
fn wrap(action:fn()->Future<Unit>)->Handler{return Handler(action:action);}
fn invoke(handler:Handler)->Future<Unit>{return handler.action();}
fn create(problem:Error)->fn()->Future<Unit>{return async fn(){throw problem;};}
async fn main(){let handler=wrap(create(A(message:"captured")));try{await invoke(handler);}catch[A]{}}
"#,
    );
}

#[test]
fn enum_discriminants_select_future_results_and_synchronous_errors() {
    accepted(
        r#"
enum Choice{First,Second}
fn choose(value:Choice)->Future<Unit>{switch value{case Choice.First:{return a();}case Choice.Second:{return b();}}}
async fn main(){try{await choose(Choice.First());}catch[A]{}}
"#,
    );
    incomplete(
        r#"
enum Choice{First,Second}
fn choose(value:Choice)->Future<Unit>{switch value{case Choice.First:{return a();}case Choice.Second:{return b();}}}
async fn main(){try{await choose(Choice.Second());}catch[A]{}}
"#,
    );
    accepted(
        r#"
enum Choice{First,Second}
fn raise(value:Choice){switch value{case Choice.First:{throw A(message:"a");}case Choice.Second:{throw B(message:"b");}}}
fn relay(value:Choice){raise(value);}
async fn main(){try{relay(Choice.First());}catch[A]{}}
"#,
    );
}

#[test]
fn callable_success_values_preserve_selected_error_values() {
    accepted(
        r#"
async fn makeA()->Error{return A(message:"a");}
async fn makeB()->Error{return B(message:"b");}
fn invoke(action:fn()->Future<Error>)->Future<Error>{return action();}
async fn main(){let problem=await invoke(makeA);try{throw problem;}catch[A]{}}
"#,
    );
}

#[test]
fn unknown_and_mutable_function_sources_keep_all_possible_failures() {
    incomplete(
        r#"
class Handler{action:fn()->Future<Unit>}
fn invoke(handler:Handler)->Future<Unit>{return handler.action();}
async fn main(){let handler=Handler(action:a);let alias=handler;alias.action=b;try{await invoke(handler);}catch[A]{}}
"#,
    );
    accepted(
        r#"
class Handler{action:fn()->Future<Unit>}
fn invoke(handler:Handler)->Future<Unit>{return handler.action();}
async fn main(){let handler=Handler(action:a);let alias=handler;alias.action=b;try{await invoke(handler);}catch errors{}}
"#,
    );
}

#[test]
fn function_markers_and_private_environment_access_are_not_changed() {
    for text in [
        "fn plain(){} fn main(){let action=plain;let same=action==action;}",
        "fn plain(){} fn execute(action:fn()->Unit){action();} async fn main(){let action=plain;await spawn execute(action);}",
        "fn main(){let action=fn(){};let environment=action.environment;}",
    ] {
        assert!(
            compile_source(&SourceFile::new(
                FileId(246),
                "callable-boundaries.rinzu",
                text
            ))
            .is_err()
        );
    }
}
