use rinzu_compiler::{
    DiagnosticCode, FileId, SourceFile, compile_source, format_source, parse_source,
};
#[test]
fn raw_multiline_content_and_spelling_survive_parser_and_formatter() {
    let text = include_str!("../../../tests/fixtures/programs/raw-text/src/main.rinzu");
    let file = SourceFile::new(FileId(1), "raw.rinzu", text);
    let formatted = format_source(&file).unwrap();
    assert!(formatted.contains("header\n  日本語 \"quoted\"#\nfooter"));
    let file = SourceFile::new(FileId(1), "raw.rinzu", formatted.clone());
    assert_eq!(formatted, format_source(&file).unwrap());
    compile_source(&file).unwrap();
    for count in 0..=16 {
        let hashes = "#".repeat(count);
        let text = format!("fn main() {{let value=r{hashes}\"日\r\n\t\\u{{0}}\"{hashes};}}");
        parse_source(&SourceFile::new(FileId(2), "hash.rinzu", text)).unwrap();
    }
}
#[test]
fn invalid_delimiters_have_original_source_spans() {
    for text in [
        "r#name",
        "r#################\"x\"#################",
        "r##\"unfinished\"#",
    ] {
        let file = SourceFile::from_utf8(
            FileId(7),
            "bom.rinzu",
            [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
        )
        .unwrap();
        let errors = parse_source(&file).unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e.code,
                DiagnosticCode::InvalidToken | DiagnosticCode::UnterminatedLiteral
            ) && e.primary.start_byte == 3),
            "{errors:?}"
        );
    }
    let text = "fn main() {let x=f\"{r\"multi\nline\"}\";}";
    assert!(parse_source(&SourceFile::new(FileId(1), "f.rinzu", text)).is_err());
}
