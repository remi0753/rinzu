//! Test entries use the ordinary loader, name/type checker and verified IR.
use std::path::{Path, PathBuf};

use crate::ast::{
    Arguments, Block, Declaration, Expression, ExpressionKind, FunctionDecl, Module, Statement,
    StatementKind, TypeRef, TypeRefKind,
};
use crate::{Diagnostic, DiagnosticCode, FrontendArtifact, Manifest, SourceRange, SourceSnapshot};

#[derive(Clone, Debug, Default)]
pub struct PackageTestOptions {
    pub unit: bool,
    pub documents: Vec<PathBuf>,
    pub selected: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PackageTestCase {
    pub identity: String,
    pub kind: &'static str,
    pub path: PathBuf,
    pub range: SourceRange,
    pub is_async: bool,
    pub(crate) target: String,
}

pub fn check_package_tests(
    start: &Path,
    sources: &mut SourceSnapshot<'_>,
    options: &PackageTestOptions,
) -> Result<(Manifest, FrontendArtifact, Vec<PackageTestCase>), Vec<Diagnostic>> {
    sources.set_limits(1024 * 1024, 512);
    crate::package::check_tests(start, sources, options)
}

pub(crate) fn error(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidEntryPoint,
        range,
        message,
        "use zero-argument non-generic Unit test functions or a complete rinzu-test program",
    )
}

pub(crate) fn validate(function: &FunctionDecl) -> Result<(), Vec<Diagnostic>> {
    if function.name == "test_"
        || !function.parameters.is_empty()
        || !function.type_parameters.is_empty()
        || function.fallback.is_some()
        || !matches!(&function.result.kind, TypeRefKind::Named(name) if name == &["Unit"])
    {
        return Err(vec![error(
            function.range,
            format!("invalid test signature `{}`", function.name),
        )]);
    }
    Ok(())
}

pub(crate) fn entry(case: Option<&PackageTestCase>, range: SourceRange) -> Declaration {
    // Scope identities use source ranges. Synthetic function/body spans differ;
    // the call site carries the selected physical source range.
    let function_range = SourceRange::new(crate::FileId::MANIFEST, 0, 2);
    let body_range = SourceRange::new(crate::FileId::MANIFEST, 1, 2);
    let statements = case
        .map(|case| {
            let call = Expression {
                kind: ExpressionKind::Call {
                    target: vec![case.target.clone()],
                    arguments: Arguments::Positional(Vec::new()),
                },
                range,
            };
            vec![Statement {
                kind: StatementKind::Expression(if case.is_async {
                    Expression {
                        kind: ExpressionKind::Await(Box::new(call)),
                        range: body_range,
                    }
                } else {
                    call
                }),
                range,
            }]
        })
        .unwrap_or_default();
    Declaration::Function(FunctionDecl {
        error_contract: None,
        name: "main".into(),
        is_private: false,
        lexical_owner: None,
        owner_type_parameter_count: 0,
        type_parameters: Vec::new(),
        is_async: true,
        parameters: Vec::new(),
        result: TypeRef::unit(function_range),
        body: Block {
            statements,
            range: body_range,
        },
        fallback: None,
        range: function_range,
    })
}

pub(crate) struct DocumentModule {
    pub name: String,
    pub syntax: Module,
    pub case: PackageTestCase,
}

pub(crate) fn documents(
    paths: &[PathBuf],
    root: &Path,
    sources: &mut SourceSnapshot<'_>,
) -> Result<Vec<DocumentModule>, Vec<Diagnostic>> {
    let mut result = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for path in paths {
        let path = root.join(path).canonicalize().map_err(|e| {
            vec![error(
                SourceRange::empty(crate::FileId::MANIFEST, 0),
                e.to_string(),
            )]
        })?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let bytes = sources.read_bytes(&path).map_err(|e| {
            vec![error(
                SourceRange::empty(crate::FileId::MANIFEST, 0),
                e.to_string(),
            )]
        })?;
        let source = sources.source(&path, bytes.clone())?;
        let text = source.text();
        let mut opened: Option<(u8, usize, bool, usize, usize)> = None;
        let mut offset = 0;
        for (line_index, line) in text.split_inclusive('\n').enumerate() {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            let indent = trimmed.bytes().take_while(|b| *b == b' ').count();
            let tail = &trimmed[indent..];
            let character = tail.bytes().next().unwrap_or(0);
            let length = tail.bytes().take_while(|b| *b == character).count();
            let marker = indent <= 3 && matches!(character, b'`' | b'~') && length >= 3;
            if let Some((first, minimum, executable, start, opening_line)) = opened {
                if marker
                    && character == first
                    && length >= minimum
                    && tail[length..].trim().is_empty()
                {
                    if executable {
                        let mut padded = bytes.clone();
                        let bom = source.original_offset(0);
                        for (index, byte) in padded.iter_mut().enumerate() {
                            if index >= bom
                                && (index < start + bom || index >= offset + bom)
                                && !matches!(*byte, b'\n' | b'\r')
                            {
                                *byte = b' ';
                            }
                        }
                        let file = crate::SourceFile::from_utf8(source.id(), &path, padded)?;
                        let mut syntax = crate::parse_source(&file)?;
                        syntax.range = SourceRange::new(
                            file.id(),
                            source.original_offset(start),
                            source.original_offset(offset),
                        );
                        if !syntax.native_blocks.is_empty() {
                            return Err(vec![error(
                                syntax.native_blocks[0].range,
                                "doc examples cannot declare native blocks",
                            )]);
                        }
                        let mains: Vec<_> = syntax
                            .declarations
                            .iter()
                            .filter_map(|d| match d {
                                Declaration::Function(f) if f.name == "main" => Some(f),
                                _ => None,
                            })
                            .collect();
                        if mains.len() != 1 {
                            return Err(vec![error(
                                syntax.range,
                                "rinzu-test block needs exactly one main",
                            )]);
                        }
                        validate(mains[0])?;
                        let name = format!("rinzu_doc_case_{}", result.len());
                        let case = PackageTestCase {
                            identity: format!("doc:{}#L{}", path.display(), opening_line),
                            kind: "doc",
                            path: path.clone(),
                            range: mains[0].range,
                            is_async: mains[0].is_async,
                            target: format!("{name}::main"),
                        };
                        result.push(DocumentModule { name, syntax, case });
                        if result.len() > 256 {
                            return Err(vec![error(
                                syntax_range(&file),
                                "doc test case limit exceeded (256)",
                            )]);
                        }
                    }
                    opened = None;
                }
            } else if marker {
                let info = tail[length..].trim();
                if info.split_whitespace().next() == Some("rinzu-test") && info != "rinzu-test" {
                    return Err(vec![error(
                        SourceRange::new(
                            source.id(),
                            source.original_offset(offset),
                            source.original_offset(offset + trimmed.len()),
                        ),
                        "rinzu-test fence accepts no additional options",
                    )]);
                }
                opened = Some((
                    character,
                    length,
                    info == "rinzu-test",
                    offset + line.len(),
                    line_index + 1,
                ));
            }
            offset += line.len();
        }
        if let Some((_, _, true, start, _)) = opened {
            return Err(vec![error(
                SourceRange::empty(source.id(), source.original_offset(start)),
                "unclosed rinzu-test fence",
            )]);
        }
    }
    Ok(result)
}

fn syntax_range(file: &crate::SourceFile) -> SourceRange {
    SourceRange::new(
        file.id(),
        file.original_offset(0),
        file.original_offset(file.text().len()),
    )
}
