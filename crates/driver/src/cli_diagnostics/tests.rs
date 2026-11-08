use super::*;
use rinzu_compiler::{Applicability, DiagnosticCode as Code, compile_source, verify_module};

fn renderer() -> Renderer {
    Renderer::at(PathBuf::from("/workspace"))
}

#[test]
fn every_diagnostic_code_preserves_its_header_excerpt_notes_and_suggestions() {
    let codes = [
        (Code::InvalidUtf8, "RZ0001"),
        (Code::InvalidToken, "RZ0002"),
        (Code::UnterminatedComment, "RZ0003"),
        (Code::UnterminatedLiteral, "RZ0004"),
        (Code::InvalidEscape, "RZ0005"),
        (Code::IntegerOutOfRange, "RZ0006"),
        (Code::UnexpectedToken, "RZ0101"),
        (Code::ExpectedToken, "RZ0102"),
        (Code::DuplicateName, "RZ0201"),
        (Code::UnknownName, "RZ0202"),
        (Code::UnknownType, "RZ0203"),
        (Code::TypeMismatch, "RZ0301"),
        (Code::InvalidOperator, "RZ0302"),
        (Code::InvalidAssignment, "RZ0303"),
        (Code::InvalidControlFlow, "RZ0304"),
        (Code::InvalidConstant, "RZ0310"),
        (Code::InvalidAwait, "RZ0401"),
        (Code::InvalidSpawn, "RZ0402"),
        (Code::InvalidFinally, "RZ0403"),
        (Code::InvalidEntryPoint, "RZ0404"),
        (Code::NonExhaustiveSwitch, "RZ0405"),
        (Code::FutureDiscarded, "RZ0406"),
        (Code::UnsupportedSafeFfi, "RZ0407"),
        (Code::CallOnlyEscape, "RZ0408"),
        (Code::InvalidHandlerReference, "RZ0409"),
        (Code::UnsupportedLanguageFeature, "RZ0410"),
        (Code::InvalidCatchPattern, "RZ0411"),
        (Code::NonExhaustiveCatch, "RZ0412"),
        (Code::ErrorEscapesBoundary, "RZ0413"),
        (Code::InvalidErrorContract, "RZ0414"),
        (Code::InvalidInterface, "RZ0415"),
        (Code::IrVerification, "RZ0501"),
        (Code::ManifestNotFound, "RZ0601"),
        (Code::ManifestInvalid, "RZ0602"),
        (Code::PackageIo, "RZ0603"),
        (Code::ImportCycle, "RZ0604"),
        (Code::InvalidModulePath, "RZ0605"),
    ];
    let source = SourceFile::new(FileId(1), "/workspace/main.rinzu", "first\n  input\n");
    let sdk = SourceFile::new(FileId(2), "/sdk/source.rinzu", "declaration\n");
    let replace = SourceRange::new(sdk.id(), 1, 3);
    for (code, spelling) in codes {
        let diagnostic = Diagnostic::error(
            code,
            SourceRange::new(source.id(), 8, 13),
            "message",
            "explanation",
        )
        .with_secondary(SourceRange::new(sdk.id(), 0, 11), "definition")
        .with_secondary(SourceRange::empty(FileId(3), 0), "file only")
        .with_secondary(SourceRange::empty(FileId(4), 0), "location unavailable")
        .with_replacement(
            Applicability::MachineApplicable,
            replace,
            "value",
            "replace the declaration",
        );
        let text = renderer().render(&diagnostic, |id| match id.0 {
            1 => Some(Input::Source(&source)),
            2 => Some(Input::Source(&sdk)),
            3 => Some(Input::Bytes(Path::new("/workspace/data.txt"), None)),
            _ => None,
        });
        assert_eq!(
            text,
            format!(
                "main.rinzu:2:3: {spelling}: message\n  2 |   input\n    |   ^^^^^\n  note: /sdk/source.rinzu:1:1: definition\n  note: data.txt: file only\n  note: location unavailable\n  help: explanation\n  help: /sdk/source.rinzu:1:2: replace the declaration\n",
            )
        );
    }
}

#[test]
fn file_level_and_unavailable_ranges_do_not_turn_into_eof_insertions() {
    let renderer = renderer();
    let file = SourceFile::new(FileId(1), "/workspace/empty.rinzu", "");
    let diagnostic = Diagnostic::error(
        Code::PackageIo,
        SourceRange::empty(file.id(), 0),
        "failure",
        "try again",
    );
    let path_only = diagnostic.clone().with_path("/workspace/missing.rinzu");
    let resolve = |_| Some(Input::Source(&file));
    assert_eq!(
        renderer.render(&path_only, resolve),
        "missing.rinzu: RZ0603: failure\n  help: try again\n"
    );
    assert_eq!(
        renderer.render(&diagnostic, |_| None),
        "RZ0603: failure\n  help: try again\n"
    );
    assert_eq!(
        renderer.render(&diagnostic, resolve),
        "empty.rinzu:1:1: RZ0603: failure\n  1 | \n    | ^\n  help: try again\n"
    );

    let mut location = String::new();
    renderer.location(&mut location, Path::new("/workspace/log"), Some(12), None);
    assert_eq!(location, "log:12: ");
}

#[test]
fn excerpts_mark_the_first_line_and_quote_width_dependent_ranges() {
    let render = |text: &str, start, end| {
        let file = SourceFile::from_utf8(
            FileId(1),
            "<formatted names.rinzu>",
            text.as_bytes().to_vec(),
        )
        .unwrap();
        renderer().render(
            &Diagnostic::error(
                Code::ExpectedToken,
                SourceRange::new(file.id(), start, end),
                "unexpected",
                "fix",
            ),
            |_| Some(Input::Source(&file)),
        )
    };
    assert_eq!(
        render("abc\ndef", 1, 5),
        "<formatted names.rinzu>:1:2: RZ0102: unexpected\n  1 | abc\n    |  ^^\n  range continues through line 2\n  help: fix\n"
    );
    assert_eq!(
        render("\t😀x", 5, 6),
        "<formatted names.rinzu>:1:3: RZ0102: unexpected\n  1 | \t😀x\n  range: \"x\"\n  help: fix\n"
    );
    assert_eq!(
        render("\u{feff}\t日本😀x\r\nnext\r\n", 4, 19),
        "<formatted names.rinzu>:1:2: RZ0102: unexpected\n  1 | \t日本😀x\n  range: \"日本😀x\"\n  range continues through line 2\n  help: fix\n"
    );
    assert_eq!(
        render("abc\r\ndef", 1, 5),
        "<formatted names.rinzu>:1:2: RZ0102: unexpected\n  1 | abc\n    |  ^^\n  help: fix\n"
    );
    let long = format!("{}X{}", "a".repeat(150), "b".repeat(150));
    let rendered = render(&long, 150, 151);
    assert_eq!(
        rendered,
        format!(
            "<formatted names.rinzu>:1:151: RZ0102: unexpected\n  1 | ...{}X{}...\n    | {}^\n  help: fix\n",
            "a".repeat(40),
            "b".repeat(79),
            " ".repeat(43),
        )
    );
}

#[test]
fn ir_verification_errors_render_against_the_originating_source() {
    let file = SourceFile::new(FileId(1), "/workspace/main.rinzu", "fn main() {}\n");
    let mut ir = compile_source(&file).unwrap().ir;
    ir.functions.push(ir.functions[0].clone());
    let errors = verify_module(&ir).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    let diagnostic = &errors[0];
    let text = renderer().render(diagnostic, |_| Some(Input::Source(&file)));
    assert!(
        text.starts_with("main.rinzu:1:1: RZ0501: duplicate function name `main`\n"),
        "{text}"
    );
    assert!(text.contains("  1 | fn main() {}\n    | ^^^^^^^^^^^^\n  note: main.rinzu:1:1: first function is here\n  help: "), "{text}");
}

#[test]
fn compiler_blocks_keep_their_boundaries_when_forwarded() {
    let blocks = [
        renderer().stage(
            Some(Path::new("/workspace/sdk/shim.c")),
            "native compilation",
            Some("clang"),
            "sdk/shim.c:1:9: error: invalid\n1 error generated.\n",
        ),
        renderer().stage(
            None,
            "AOT linking",
            Some("clang"),
            "ld: undefined symbol\n  detail\n",
        ),
    ];
    let (_, forwarded) = capture(|| {
        let (_, child) = capture(|| {
            for block in &blocks {
                write_block(block);
            }
        });
        for block in child {
            write_block(&block);
        }
    });
    assert_eq!(
        forwarded,
        [
            "sdk/shim.c: error: native compilation failed (clang)\n  sdk/shim.c:1:9: error: invalid\n  1 error generated.\n",
            "error: AOT linking failed (clang)\n  ld: undefined symbol\n    detail\n",
        ]
    );
}

#[test]
fn call_only_boundary_diagnostic_uses_the_checked_payload_span() {
    let file = SourceFile::new(
        FileId(1),
        "/workspace/native.rinzu",
        "error E { payload: Borrowed } fn main() {}",
    );
    let mut options = rinzu_compiler::CompilerOptions::default();
    options.external_types.insert(
        "Borrowed".into(),
        rinzu_compiler::ExternalType { call_only: true },
    );
    let syntax = rinzu_compiler::parse_source(&file).unwrap();
    let errors = rinzu_compiler::check_module_with_options(&syntax, &options).unwrap_err();
    let diagnostic = errors
        .iter()
        .find(|error| error.code == Code::CallOnlyEscape)
        .unwrap();
    let text = renderer().render(diagnostic, |_| Some(Input::Source(&file)));
    assert!(
        text.starts_with(
            "native.rinzu:1:11: RZ0408: Error payload cannot retain a borrowed or call-only value\n"
        ),
        "{text}"
    );
    assert!(text.contains("\n  1 | error E { payload: Borrowed } fn main() {}\n    |           ^^^^^^^^^^^^^^^^^\n  help: "), "{text}");
}
