use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source, format_source};
fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "try-all.rinzu", text)
}
#[test]
fn try_all_uses_statement_edges_unbound_reads_and_list_catch() {
    let text = include_str!("../../../tests/fixtures/programs/try-all/src/main.rinzu");
    let formatted = format_source(&file(text)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    let artifact = compile_source(&file(&formatted)).unwrap();
    let operations = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .collect::<Vec<_>>();
    assert!(
        operations
            .iter()
            .any(|op| matches!(op.kind, OperationKind::RecordTaskError { .. }))
    );
    assert!(
        operations
            .iter()
            .any(|op| matches!(op.kind, OperationKind::CheckLocal { .. }))
    );
    assert!(
        operations
            .iter()
            .any(|op| matches!(op.kind, OperationKind::FinishErrorGroup { .. }))
    );
    for text in [
        "async fn child(){} async fn main(){try all {let pending=child();await pending;} catch errors {let count=listLength(errors);}}",
        "async fn main(){try{try all {let bad=CInt8(300);} finally {}}catch errors{}}",
        "async fn main(){try all {try all {let bad=CInt8(300);} catch inner {}} catch outer {}}",
        "async fn child(){} async fn main(){child();}",
    ] {
        compile_source(&file(text)).unwrap();
    }
    for (text, message) in [
        (
            "error Failed{} async fn child(){throw Failed(message:\"failure\");} async fn main(){child();}",
            "observation path",
        ),
        (
            "async fn main(){try all {} catch errors {let code=errorCode(errors);}}",
            "List<Error>",
        ),
        ("async fn main(){try all {} }", "finally"),
    ] {
        let errors = compile_source(&file(text)).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.to_lowercase().contains(&message.to_lowercase())),
            "{text}: {errors:?}"
        );
    }
}
