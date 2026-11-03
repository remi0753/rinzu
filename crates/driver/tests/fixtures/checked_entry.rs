//! Success-only CLI fixtures still handle normal Error at their Main boundary.
//! Parsing keeps braces inside strings, interpolation and nested declarations
//! separate from the actual function body. Failing Futures still need their own
//! observation path; this handler cannot adopt or implicitly join them.
pub fn source(source: &str) -> String {
    let file = rinzu_compiler::SourceFile::new(rinzu_compiler::FileId(1), "fixture.rinzu", source);
    let module = rinzu_compiler::parse_source(&file).unwrap();
    let function = module
        .declarations
        .iter()
        .find_map(|declaration| {
            if let rinzu_compiler::ast::Declaration::Function(function) = declaration
                && function.name == "main"
            {
                Some(function)
            } else {
                None
            }
        })
        .expect("fixture Main");
    let start = function.body.range.start_byte as usize;
    let end = function.body.range.end_byte as usize;
    assert_eq!(&source[start..start + 1], "{");
    assert_eq!(&source[end - 1..end], "}");
    format!(
        "{}{{try{{{}}}catch unexpectedFixtureErrors{{panic(\"unexpectedFixtureFailure\");}}}}{}",
        &source[..start],
        &source[start + 1..end - 1],
        &source[end..],
    )
}
