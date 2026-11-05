use rinzu_compiler::{FileId, SourceFile, compile_source, format_source};

#[test]
fn revised_syntax_formats_idempotently_and_retains_unicode_literals_and_comments() {
    let text = "import rinzu.list;\n// 日本語\r\ntype Pair={x:Int,y:Int};class Counter{const tag:String,value:Int fn read(self:Counter)->Int{return self.value;}}native{record Input{value:CInt}}async fn main(){var base:Int=40;let p:Pair={y=2,x=1};let c=Counter(tag:\"日本\",value:0);let add=fn(a:Int)->Int{return base+a;};let later=async fn()->Int{return add(p.x);};try all{let n=await later();c.value=n;}catch errors{let count=rinzu.list.length(errors);}finally{let text=f\"日本-{c.value}\";}}";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend(text.as_bytes());
    let source = SourceFile::from_utf8(FileId(19), "tooling.rinzu", bytes).unwrap();
    let original = compile_source(&source).unwrap();
    let formatted = format_source(&source).unwrap();
    let formatted_source = SourceFile::new(FileId(19), "formatted.rinzu", &formatted);
    assert_eq!(formatted, format_source(&formatted_source).unwrap());
    let after = compile_source(&formatted_source).unwrap();
    assert!(formatted.contains("// 日本語") && formatted.contains("f\"日本-{c.value}\""));
    assert_eq!(
        original.ir.source_types["Counter"].const_fields,
        after.ir.source_types["Counter"].const_fields
    );
    assert_eq!(
        original
            .ir
            .source_types
            .keys()
            .filter(|s| s.starts_with("$rinzu_record_"))
            .collect::<Vec<_>>(),
        after
            .ir
            .source_types
            .keys()
            .filter(|s| s.starts_with("$rinzu_record_"))
            .collect::<Vec<_>>()
    );
    for function in original
        .checked
        .functions
        .iter()
        .filter(|f| f.declaration.name.contains("$rinzu_lambda_"))
    {
        let range = function.declaration.range;
        assert_eq!(range.file_id, FileId(19));
        assert!(
            text.as_bytes()[range.start_byte as usize - 3..range.end_byte as usize - 3]
                .starts_with(b"fn(")
                || text.as_bytes()[range.start_byte as usize - 3..range.end_byte as usize - 3]
                    .starts_with(b"async fn(")
        );
    }
}

#[test]
fn structural_field_diagnostics_point_into_original_bom_crlf_bytes() {
    let text = "// 日本語\r\ntype Pair={x:Int,y:Int};fn main(){let p:Pair={x=\"bad\",y=1};}";
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend(text.as_bytes());
    let errors =
        compile_source(&SourceFile::from_utf8(FileId(27), "field.rinzu", bytes.clone()).unwrap())
            .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.primary.file_id == FileId(27)
                && &bytes[error.primary.start_byte as usize..error.primary.end_byte as usize]
                    == b"\"bad\""),
        "{errors:?}"
    );
}
