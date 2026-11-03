use rinzu_compiler::{FileId, SourceFile, compile_source, parse_source};

#[test]
fn removed_surfaces_explain_the_revised_alternative_at_the_original_token() {
    for (text, removed, alternative) in [
        (
            "record Point {x:Int} fn main(){}",
            "named record/state",
            "structural type alias",
        ),
        (
            "state Point {x:Int} fn main(){}",
            "named record/state",
            "class",
        ),
        (
            "fn add(x:Int,y:Int)->Int{return x+y;} fn main(){let f=bind add(40);}",
            "bind expressions",
            "fn(",
        ),
        (
            "fn main(){let p={x:Int=1};let q=p with {x:2};}",
            "with expressions",
            "every field",
        ),
        (
            "async fn main(){taskScope {} finally {}}",
            "taskScope",
            "try all",
        ),
    ] {
        let errors = parse_source(&SourceFile::new(FileId(3), "removed.rinzu", text)).unwrap_err();
        let error = &errors[0];
        assert!(
            error.message.contains(removed) && error.message.contains("0.5"),
            "{errors:?}"
        );
        assert!(
            error
                .suggestions
                .iter()
                .any(|s| s.message.contains(alternative)),
            "{errors:?}"
        );
        let token = &text[error.primary.start_byte as usize..error.primary.end_byte as usize];
        assert!(matches!(
            token,
            "record" | "state" | "bind" | "with" | "taskScope"
        ));
    }
}

#[test]
fn native_layout_record_and_contextual_identifier_names_survive_the_migration() {
    compile_source(&SourceFile::new(FileId(1), "native.rinzu", "native {record Input {value:CInt}} fn read(input:Input)->CInt{return input.value;} fn bind(state:Int)->Int{return state;} fn main(){try{let state=bind(1);let input=Input(value:CInt(state));let n=read(input);}catch errors{}}" )).unwrap();
    let text =
        "class Point {const x:Int} fn Point.read(self:Point)->Int{return self.x;} fn main(){}";
    assert!(
        compile_source(&SourceFile::new(FileId(1), "method.rinzu", text))
            .unwrap_err()
            .iter()
            .any(|e| e.message.contains("inside class"))
    );
}
