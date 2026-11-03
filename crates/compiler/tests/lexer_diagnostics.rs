use rinzu_compiler::{
    DiagnosticCode, FileId, SourceFile, compile_source, format_source, parse_source,
};

#[test]
fn trailing_escape_cannot_hide_an_unterminated_literal_after_valid_source() {
    for prefix in ["\"", "f\"", "b\""] {
        for bom in [false, true] {
            let text = format!("fn main() {{}} {prefix}unfinished\\");
            let mut bytes = if bom {
                vec![0xef, 0xbb, 0xbf]
            } else {
                Vec::new()
            };
            bytes.extend_from_slice(text.as_bytes());
            let file = SourceFile::from_utf8(FileId(7), "unfinished.rinzu", bytes).unwrap();
            let errors = parse_source(&file).expect_err("unfinished literal must fail to parse");
            assert!(
                errors.iter().any(|error| {
                    error.code == DiagnosticCode::UnterminatedLiteral
                        && error.primary.start_byte as usize
                            == file.original_offset(text.find(prefix).unwrap())
                        && error.primary.end_byte as usize == file.original_offset(text.len())
                }),
                "{prefix:?}, BOM={bom}: {errors:#?}"
            );
            assert!(compile_source(&file).is_err());
            assert!(format_source(&file).is_err());
        }
    }
}

#[test]
fn invalid_unicode_tokens_cover_one_complete_scalar_each() {
    let text = "fn main() { 日本😀; }";
    for bom in [false, true] {
        let mut bytes = if bom {
            vec![0xef, 0xbb, 0xbf]
        } else {
            Vec::new()
        };
        bytes.extend_from_slice(text.as_bytes());
        let file = SourceFile::from_utf8(FileId(7), "unicode.rinzu", bytes).unwrap();
        let errors = parse_source(&file).unwrap_err();
        assert_eq!(errors.len(), 3, "one error per scalar: {errors:#?}");
        for (error, character) in errors.iter().zip(['日', '本', '😀']) {
            let start = file.original_offset(text.find(character).unwrap());
            let end = start + character.len_utf8();
            assert_eq!(error.code, DiagnosticCode::InvalidToken);
            assert_eq!(error.primary.start_byte as usize, start);
            assert_eq!(error.primary.end_byte as usize, end);
            let start_position = file.utf16_position(start).unwrap();
            let end_position = file.utf16_position(end).unwrap();
            assert_eq!(
                end_position.character - start_position.character,
                character.len_utf16() as u32
            );
        }
    }
}

#[test]
fn invalid_unicode_escapes_keep_diagnostic_endpoints_on_character_boundaries() {
    for text in [
        "fn main() { let x = \"\\😀\"; }",
        "fn main() { let x = f\"\\😀\"; }",
        "fn main() { let x = b\"é\"; }",
        "fn main() { let x = b\"\\é\"; }",
    ] {
        let file = SourceFile::new(FileId(7), "escape.rinzu", text);
        let errors = parse_source(&file).unwrap_err();
        assert_eq!(errors.len(), 1, "{text}: {errors:#?}");
        assert_eq!(errors[0].code, DiagnosticCode::InvalidEscape);
        for error in errors {
            for offset in [error.primary.start_byte, error.primary.end_byte] {
                assert!(
                    file.utf16_position(offset as usize).is_ok(),
                    "{text}: {error:#?}"
                );
                let _ = file.line_column(offset as usize);
            }
        }
    }
}
