use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};
const BASE: &str = r#"
import rinzu.List;
error Failed{}
async fn fail(){throw Failed(message:"failure");}
async fn safe(){}
async fn receive(value:Future<Unit>){try{await value;}catch errors{}}
class Cell{future:Future<Unit>}
async fn fromCell(cell:Cell){try{await cell.future;}catch errors{}}
fn overwrite(cells:List<Cell>){var index=0;while index<rinzu.List.length(cells){switch rinzu.List.get(cells,index){case Option.Some(cell):{cell.future=safe();}case Option.None:{}}index=index+1;}}
"#;
fn verify(body: &str, valid: bool) {
    let result = compile_source(&SourceFile::new(
        FileId(275),
        "receiver-coverage.rinzu",
        format!("{BASE}{body}"),
    ));
    match result {
        Ok(_) => assert!(valid, "partial coverage was accepted"),
        Err(errors) => {
            assert!(!valid, "{errors:#?}");
            assert!(
                errors
                    .iter()
                    .any(|error| error.code == DiagnosticCode::FutureDiscarded),
                "{errors:#?}"
            );
        }
    }
}
const LOOP: &str = "async fn main(){var cells:List<Cell> = [];var index=0;while index<3{let pending=fail();let cell=Cell(future:pending);let field=fromCell(cell);RECEIVE cells=rinzu.List.append(cells,cell);index=index+1;}overwrite(cells);}";
#[test]
fn a_partial_receiver_family_cannot_replace_all_field_receivers() {
    verify(
        &LOOP.replace("RECEIVE", "if index==0{let value=receive(pending);}"),
        false,
    );
}
#[test]
fn a_complete_receiver_family_can_replace_all_field_receivers() {
    verify(
        &LOOP.replace("RECEIVE", "let value=receive(pending);"),
        true,
    );
}
