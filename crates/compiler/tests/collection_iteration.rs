use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source, format_source};

fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId(301), "collection-iteration.rinzu", source)
}

#[test]
fn snapshot_iteration_is_checked_and_formats_idempotently() {
    let text = "fn for(x:Int)->Int{return x;} fn main(){var sum=0;var values=[1,2,3];for value in values {values=[9];if value==2{continue;}sum=sum+for(value);if sum==4{break;}}for _ in values {} }";
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .flat_map(|f| &f.blocks)
            .flat_map(|b| &b.operations)
            .any(|o| matches!(o.kind, rinzu_compiler::OperationKind::ListElement { .. }))
    );
}

#[test]
fn binding_is_readonly_and_source_must_be_a_list() {
    for text in [
        "fn main(){for value in [1,2]{value=3;}}",
        "fn main(){for value in 3{}}",
        "fn main(){for value in [1]{}let invalid=value;}",
    ] {
        assert!(compile_source(&file(text)).is_err(), "{text}");
    }
}

#[test]
fn editor_resolves_the_loop_binding_only_inside_its_body() {
    let text = "fn main(){let values=[1];for current in values{let inside=current;}let outside=0;}";
    let input = file(text);
    let syntax = rinzu_compiler::parse_source(&input).unwrap();
    let sources = std::collections::BTreeMap::from([(input.path().to_owned(), input.clone())]);
    let index = rinzu_compiler::build_editor_index(
        &sources,
        &[(input.clone(), syntax.clone())],
        &[&syntax],
    );
    let binding = text.find("current in").unwrap();
    let usage = text.find("=current").unwrap() + 1;
    assert_eq!(
        index
            .definition_at(rinzu_compiler::SourceRange::empty(input.id(), usage))
            .unwrap()
            .start_byte as usize,
        binding
    );
    assert!(
        index
            .visible_symbols(rinzu_compiler::SourceRange::empty(input.id(), usage))
            .iter()
            .any(|s| s.name == "current")
    );
    assert!(
        !index
            .visible_symbols(rinzu_compiler::SourceRange::empty(
                input.id(),
                text.find("outside").unwrap()
            ))
            .iter()
            .any(|s| s.name == "current")
    );
}

const TASKS: &str = "error Failed{} async fn fail()->Int{throw Failed(message:\"failure\");}";

#[test]
fn complete_traversal_observes_all_future_elements() {
    for body in [
        "try all{for pending in requests{let received=await pending;}}catch errors{}",
        "for pending in requests{try{let received=await pending;}catch errors{}continue;}",
    ] {
        let text = format!(
            "{TASKS} async fn observe(requests:List<Future<Int>>){{{body}}} async fn main(){{let requests=[fail(),fail()];await observe(requests);}}"
        );
        compile_source(&file(&text)).unwrap();
    }
}

#[test]
fn early_exit_skip_and_discard_do_not_observe_the_population() {
    for body in [
        "for _ in requests{}",
        "for pending in requests{break;}",
        "for pending in requests{if futureReady(pending){continue;}try{let received=await pending;}catch errors{}}",
    ] {
        let text = format!("{TASKS} async fn main(){{let requests=[fail(),fail()];{body}}}");
        let errors = compile_source(&file(&text)).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == DiagnosticCode::FutureDiscarded),
            "{errors:#?}"
        );
    }
}
