use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source, format_source};
fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(306), "record-spread.rinzu", text)
}
#[test]
fn structural_update_preserves_the_shape_and_formats() {
    let text = "type Point={x:Int,y:Int};fn move(point:Point)->Point{return {...point,x=point.x+1};}fn main(){let before:Point={x=1,y=2};let after=move(before);}";
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file(&formatted)).unwrap();
    for update in [
        "{...point,x=1,x=2}",
        "{...point,z=1}",
        "{...point,x=\"bad\"}",
        "{...point}",
        "{...point,}",
        "{...point,...point,x=1}",
    ] {
        let text = format!(
            "type Point={{x:Int,y:Int}};fn main(){{let point:Point={{x=1,y=2}};let after={update};}}"
        );
        assert!(compile_source(&file(&text)).is_err(), "{text}");
    }
    assert!(
        compile_source(&file(
            "class C{const x:Int}fn main(){let c=C(x:1);let next={...c,x=2};}"
        ))
        .is_err()
    );
}
#[test]
fn inherited_and_overwritten_future_identities_are_not_discarded() {
    let base = "error Failed{} async fn fail()->Int{throw Failed(message:\"failed\");}";
    let text = format!(
        "{base} async fn main(){{let first=fail();let holder={{pending=first}};let next={{...holder,pending=first}};try{{let value=await next.pending;}}catch errors{{}}}}"
    );
    compile_source(&file(&text)).unwrap();
    for body in [
        "let first=fail();let holder={pending=first};let next={...holder,pending=fail()};try{let value=await next.pending;}catch errors{}",
        "let holder={flag=false,pending=fail()};try{let next={...holder,pending=fail(),flag=reject()};}catch errors{}",
    ] {
        let text = format!(
            "{base} fn reject()->Bool{{throw Failed(message:\"rejected\");}}async fn main(){{{body}}}"
        );
        let errors = compile_source(&file(&text)).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == DiagnosticCode::FutureDiscarded),
            "{errors:?}"
        );
    }
}
