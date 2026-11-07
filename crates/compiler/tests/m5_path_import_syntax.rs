use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, format_source, parse_source};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(23), "entry.rinzu", text)
}

fn formatted(text: &str) -> String {
    format_source(&source(text)).unwrap()
}

#[test]
fn path_literal_alias_and_dotted_imports_retain_separate_original_ranges() {
    let text =
        formatted(r#"import rinzu.core;import "./lib/日本語 file\u{2e}rinzu" as Math;fn main(){}"#);
    let module = parse_source(&source(&text)).unwrap();
    assert_eq!(module.imports[0].path, ["rinzu", "core"]);
    assert!(module.imports[0].file.is_none());
    let import = &module.imports[1];
    assert_eq!(import.path, ["Math"]);
    let file = import.file.as_ref().unwrap();
    assert_eq!(file.path, "./lib/日本語 file.rinzu");
    assert_eq!(
        &text[file.path_range.start_byte as usize..file.path_range.end_byte as usize],
        r#""./lib/日本語 file\u{2e}rinzu""#,
    );
    assert_eq!(
        &text[file.alias_range.start_byte as usize..file.alias_range.end_byte as usize],
        "Math",
    );
    assert_eq!(file.path_range.file_id, FileId(23));
    assert!(import.range.start_byte < file.path_range.start_byte);
    assert!(file.alias_range.end_byte < import.range.end_byte);
}

#[test]
fn bom_crlf_vector_keeps_original_byte_offsets_and_contextual_as() {
    // Deliberate encoding/span vector; do not normalize its bytes with the formatter.
    let bytes = b"\xef\xbb\xbfimport \"./lib.rinzu\" as model;\r\nfn as(){}\r\n".to_vec();
    let file = SourceFile::from_utf8(FileId(9), "bom.rinzu", bytes.clone()).unwrap();
    let module = parse_source(&file).unwrap();
    let import = &module.imports[0];
    assert_eq!(import.range.start_byte, 3);
    let path = import.file.as_ref().unwrap();
    assert_eq!(
        &bytes[path.path_range.start_byte as usize..path.path_range.end_byte as usize],
        b"\"./lib.rinzu\"",
    );
    assert_eq!(module.declarations[0].name(), "as");
}

#[test]
fn invalid_path_and_alias_vectors_report_the_violating_tokens() {
    // Deliberately invalid grammar/literal vectors cannot be formatted.
    for text in [
        "import \"\" as m;",
        "import \"./m.txt\" as m;",
        "import \"./m.RINZU\" as m;",
        "import \"./m\\u{0}.rinzu\" as m;",
        "import \"./m\\n.rinzu\" as m;",
    ] {
        let error = parse_source(&source(text)).unwrap_err().remove(0);
        assert_eq!(error.code, DiagnosticCode::InvalidModulePath, "{text}");
        assert_eq!(error.primary.start_byte, 7, "{text}");
    }
    for text in [
        "import f\"./m.rinzu\" as m;",
        "import b\"./m.rinzu\" as m;",
        "import \"./m.rinzu\";",
        "import \"./m.rinzu\" as;",
        "import \"./m.rinzu\" as fn;",
        "import \"./m.rinzu\" as a.b;",
        "import \"./m.rinzu\" as m",
        "import model as;",
        "import model as a.b;",
    ] {
        assert!(parse_source(&source(text)).is_err(), "accepted {text}");
    }
    let reserved = parse_source(&source("import \"./m.rinzu\" as rinzu;"))
        .unwrap_err()
        .remove(0);
    assert_eq!(reserved.code, DiagnosticCode::InvalidModulePath);
    assert_eq!(reserved.primary.start_byte, 22);
    let dotted = parse_source(&source("import model as rinzu;"))
        .unwrap_err()
        .remove(0);
    assert_eq!(dotted.code, DiagnosticCode::InvalidModulePath);
    assert_eq!(dotted.primary.start_byte, 16);
}

#[test]
fn formatting_retains_literal_spelling_aliases_and_comments() {
    let input = r#"// before
import/*path*/"./日本語\u{2e}rinzu"/*alias*/as first; // after
import "./日本語.rinzu" as second;
import example.callbacks/*qualifier*/as cb;
fn main(){}"#;
    let output = formatted(input);
    for spelling in [
        "// before",
        "/*path*/",
        r#""./日本語\u{2e}rinzu""#,
        "/*alias*/",
        "// after",
        "as first;",
        "as second;",
        "/*qualifier*/",
        "as cb;",
    ] {
        assert!(output.contains(spelling), "missing {spelling}: {output}");
    }
    assert_eq!(formatted(&output), output);
    let imports = parse_source(&source(&output)).unwrap().imports;
    assert_eq!(imports[0].file.as_ref().unwrap().path, "./日本語.rinzu");
    assert_eq!(imports[0].path, ["first"]);
    assert_eq!(imports[1].path, ["second"]);
    assert_eq!(imports[2].path, ["example", "callbacks"]);
    assert_eq!(imports[2].qualifier(), ["cb"]);
    let range = imports[2].alias_range().unwrap();
    assert_eq!(
        &output[range.start_byte as usize..range.end_byte as usize],
        "cb"
    );
}

#[test]
fn builtin_import_aliases_work_in_the_in_memory_compiler() {
    let source = source(
        "import rinzu.list as lists;import rinzu.core as core;fn main(){let count=lists.length([1]);let size=core.utf8ByteLength(\"x\");let lists={length:Int=42};let field=lists.length;}",
    );
    rinzu_compiler::compile_source(&source).unwrap();
}

#[test]
fn long_import_wraps_at_token_boundaries_and_indivisible_paths_remain_intact() {
    let path = format!("./{}.rinzu", "segment".repeat(12));
    let alias = "longAlias".repeat(5);
    let output = formatted(&format!("import \"{path}\" as {alias};fn main(){{}}"));
    assert!(
        output.lines().all(|line| line.chars().count() <= 120),
        "{output}"
    );
    assert!(
        output.lines().count() > 3,
        "long import did not wrap: {output}"
    );
    assert!(output.contains(&format!("\"{path}\"")));
    assert_eq!(formatted(&output), output);
    let enormous = format!("./{}.rinzu", "long".repeat(40));
    let output = formatted(&format!("import \"{enormous}\" as m;"));
    assert!(output.contains(&format!("\"{enormous}\"")));
    assert_eq!(formatted(&output), output);
}
