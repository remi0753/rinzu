//! Test entries use the ordinary loader, name/type checker and verified IR.
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::ast::{
    Arguments, Block, Declaration, Expression, ExpressionKind, FunctionDecl, Module, Statement,
    StatementKind, TypeRef, TypeRefKind,
};
use crate::{Diagnostic, DiagnosticCode, FrontendArtifact, Manifest, SourceRange, SourceSnapshot};

#[derive(Clone, Debug, Default)]
pub struct PackageTestOptions {
    pub documents: Vec<PathBuf>,
    pub selected: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) enum TestInput<'a> {
    Documents(&'a PackageTestOptions),
    File(&'a Path),
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
    crate::package::check_tests(start, sources, TestInput::Documents(options))
}

pub fn check_package_test_file(
    root: &Path,
    file: &Path,
    sources: &mut SourceSnapshot<'_>,
) -> Result<(Manifest, FrontendArtifact, PackageTestCase), Vec<Diagnostic>> {
    let (manifest, artifact, mut cases) =
        crate::package::check_tests(root, sources, TestInput::File(file))?;
    Ok((
        manifest,
        artifact,
        cases.pop().expect("checked one test file"),
    ))
}

pub fn test_source_root(root: &Path) -> io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let tests = root.join("tests").canonicalize()?;
    let source = root.join("src");
    let source = source.canonicalize().unwrap_or(source);
    if !tests.is_dir()
        || tests == root
        || !tests.starts_with(&root)
        || tests.starts_with(&source)
        || source.starts_with(&tests)
    {
        return Err(io::Error::other(
            "tests must be a directory inside the package, separate from src",
        ));
    }
    Ok(tests)
}

pub fn test_file_path(root: &Path, file: &Path) -> io::Result<String> {
    let relative = file
        .strip_prefix(root)
        .map_err(|_| io::Error::other("test file resolves outside the package tests directory"))?;
    if !fs::metadata(file)?.is_file()
        || !file
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".test.rinzu"))
    {
        return Err(io::Error::other("test entry must be a .test.rinzu file"));
    }
    let relative = relative
        .to_str()
        .ok_or_else(|| io::Error::other("test file path must be UTF-8"))?;
    Ok(format!("tests/{relative}"))
}

pub(crate) fn require_test_owner(root: &Path, path: &Path) -> Result<(), String> {
    let manifest = crate::find_manifest(path).map_err(|error| error.into_diagnostic().message)?;
    if manifest.parent() != Some(root) {
        return Err(format!(
            "test source belongs to a different package (`{}`)",
            manifest.display()
        ));
    }
    Ok(())
}

pub(crate) fn error(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::InvalidEntryPoint,
        range,
        message,
        "use a zero-argument non-generic Unit main",
    )
}

fn validate(function: &FunctionDecl) -> Result<(), Vec<Diagnostic>> {
    if !function.parameters.is_empty()
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

pub(crate) fn main_function<'a>(
    syntax: &'a Module,
    input: &str,
) -> Result<&'a FunctionDecl, Vec<Diagnostic>> {
    let mut mains = syntax
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function)
                if function.name == "main" && function.lexical_owner.is_none() =>
            {
                Some(function)
            }
            _ => None,
        });
    let Some(main) = mains.next() else {
        return Err(vec![error(
            syntax.range,
            format!("{input} needs exactly one main"),
        )]);
    };
    if let Some(other) = mains.next() {
        return Err(vec![
            error(other.range, format!("{input} needs exactly one main"))
                .with_secondary(main.range, "first main declared here"),
        ]);
    }
    validate(main)?;
    Ok(main)
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
        let requested = root.join(path);
        let path = sources.canonical_path(&requested).map_err(|e| {
            vec![
                error(
                    SourceRange::empty(crate::FileId::MANIFEST, 0),
                    e.to_string(),
                )
                .with_path(&requested),
            ]
        })?;
        if !seen.insert(path.clone()) {
            continue;
        }
        let bytes = sources.read_bytes(&path).map_err(|e| {
            vec![
                error(
                    SourceRange::empty(crate::FileId::MANIFEST, 0),
                    e.to_string(),
                )
                .with_path(&path),
            ]
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
                        let bom = source.original_offset(0);
                        let mut padded = bytes[..offset + bom].to_vec();
                        for byte in &mut padded[bom..start + bom] {
                            if !matches!(*byte, b'\n' | b'\r') {
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
                        let main = main_function(&syntax, "rinzu-test block")?;
                        let name = format!("rinzu_doc_case_{}", result.len());
                        let case = PackageTestCase {
                            identity: format!("doc:{}#L{}", path.display(), opening_line),
                            kind: "doc",
                            path: path.clone(),
                            range: main.range,
                            is_async: main.is_async,
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
