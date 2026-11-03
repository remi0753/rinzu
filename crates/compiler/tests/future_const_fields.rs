use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
error Failed{}
async fn fail(){throw Failed(message:"failure");}
async fn safe(){}
class Box{const value:Future<Unit>}
fn get(box:Box)->Future<Unit>{return box.value;}
"#;
fn check(
    source: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(
        FileId(406),
        "const-future-fields.rinzu",
        format!("{BASE}{source}"),
    ))
}
#[test]
fn const_future_fields_preserve_aliases_and_actual_failure_effects() {
    check("fn main(){let box=Box(value:safe());get(box);}").unwrap();
    check("async fn main(){let pending=fail();let box=Box(value:pending);try{await get(box);}catch errors{}}").unwrap();
    let errors = check("fn main(){let box=Box(value:fail());get(box);}").unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}
#[test]
fn a_const_reference_does_not_make_its_mutable_target_a_snapshot() {
    let declarations = "class Cell{future:Future<Unit>}class Outer{const cell:Cell}async fn receive(outer:Outer){try{await outer.cell.future;}catch errors{}}";
    check(&format!("{declarations}async fn main(){{let pending=fail();let cell=Cell(future:pending);let outer=Outer(cell:cell);let observed=receive(outer);await observed;cell.future=safe();}}" )).unwrap();
    let errors=check(&format!("{declarations}fn main(){{let pending=fail();let cell=Cell(future:pending);let outer=Outer(cell:cell);let observed=receive(outer);cell.future=safe();}}" )).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded),
        "{errors:#?}"
    );
}
#[test]
fn const_function_fields_keep_their_captured_future_retention() {
    let errors=check("class Functions{const action:fn()->Unit}error Stored{payload:Functions}fn main(){let pending=safe();let functions=Functions(action:fn(){let ready=futureReady(pending);});let stored=Stored(message:\"bad payload\",payload:functions);}").unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::TypeMismatch
                && error.message.contains("retain Future")),
        "{errors:#?}"
    );
}

#[test]
fn readonly_factory_results_keep_their_future_obligations() {
    let declarations =
        "fn build(flag:Bool)->Box{if flag{return Box(value:fail());}return Box(value:safe());}";
    check(&format!("{declarations}async fn main(){{let box=build(true);try{{await get(box);}}catch errors{{}}}}" )).unwrap();
    let errors = check(&format!(
        "{declarations}fn main(){{let box=build(true);get(box);}}"
    ))
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == DiagnosticCode::FutureDiscarded)
    );
}
