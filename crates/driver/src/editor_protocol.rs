//! Read-only, versioned JSON-lines source editor service (not an LSP transport).
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use rinzu_compiler::{
    CompilerOptions, Diagnostic, EditorIndex, FileId, Module, SourceCache, SourceFile, SourceRange,
    SourceSnapshot, Utf16Position, build_editor_index,
};
use serde_json::{Value, json};

const DOCUMENT_BYTES: usize = 1024 * 1024;
const OPEN_DOCUMENTS: usize = 32;
const SNAPSHOT_FILES: usize = 256;
const FRAME_BYTES: usize = 8 * 1024 * 1024;
const DIAGNOSTICS: usize = 256;

struct Buffer {
    version: u64,
    text: String,
    bom: bool,
}
#[derive(Default)]
struct Session {
    workspace: Option<PathBuf>,
    entry: Option<PathBuf>,
    buffers: BTreeMap<PathBuf, Buffer>,
    cache: SourceCache,
    generation: u64,
    shutdown: bool,
}
struct Failure {
    code: &'static str,
    message: String,
}
type Result<T> = std::result::Result<T, Failure>;
fn fail(code: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}
fn string<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| fail("InvalidParams", format!("{key} must be a string")))
}
fn version(params: &Value) -> Result<u64> {
    params
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| fail("InvalidParams", "version must be a nonnegative integer"))
}
fn canonical(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|e| fail("InvalidPath", e.to_string()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| fail("InvalidPath", "document path needs a file name"))?;
    Ok(parent
        .canonicalize()
        .map_err(|e| fail("InvalidPath", e.to_string()))?
        .join(name))
}
fn document_path(params: &Value) -> Result<PathBuf> {
    let path = canonical(Path::new(string(params, "path")?))?;
    if path.extension().and_then(|s| s.to_str()) != Some("rinzu") || path.is_dir() {
        return Err(fail("InvalidPath", "a document must be a .rinzu file"));
    }
    Ok(path)
}

pub fn run(arguments: impl Iterator<Item = String>) -> std::result::Result<(), String> {
    let args: Vec<_> = arguments.collect();
    if args.first().map(String::as_str) != Some("--stdio") || args.len() > 2 {
        return Err("editor requires --stdio and an optional workspace path".into());
    }
    let mut session = Session::default();
    if let Some(path) = args.get(1) {
        crate::input::require_package("editor", Path::new(path))?;
        session.workspace = Some(Path::new(path).canonicalize().map_err(|e| e.to_string())?);
    }
    serve(
        &mut session,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
    )
    .map_err(|e| e.to_string())
}

fn serve(
    session: &mut Session,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<()> {
    while let Some(frame) = read_frame(input)? {
        let response = match frame {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(request) => session.respond(&request),
                Err(error) => session.error(Value::Null, fail("ParseError", error.to_string())),
            },
            Err(()) => session.error(
                Value::Null,
                fail("LimitExceeded", "JSON frame exceeds 8 MiB"),
            ),
        };
        serde_json::to_writer(&mut *output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
        if session.shutdown {
            break;
        }
    }
    Ok(())
}

// Drain oversized lines without allocating the unbounded input line.
fn read_frame(input: &mut impl BufRead) -> io::Result<Option<std::result::Result<Vec<u8>, ()>>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    let mut seen = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(seen.then_some(if oversized { Err(()) } else { Ok(bytes) }));
        }
        seen = true;
        let end = available.iter().position(|b| *b == b'\n');
        let length = end.map_or(available.len(), |n| n + 1);
        let payload = end.unwrap_or(length);
        if !oversized {
            if bytes.len() + payload > FRAME_BYTES {
                bytes.clear();
                oversized = true;
            } else {
                bytes.extend_from_slice(&available[..payload]);
            }
        }
        input.consume(length);
        if end.is_some() {
            return Ok(Some(if oversized { Err(()) } else { Ok(bytes) }));
        }
    }
}

impl Session {
    fn envelope(&self, id: Value) -> Value {
        json!({"id":id,"schema":"rinzu-editor/1","generation":self.generation,
            "documents":self.buffers.iter().map(|(path,buffer)| json!({"path":path,"version":buffer.version})).collect::<Vec<_>>()})
    }
    fn error(&self, id: Value, failure: Failure) -> Value {
        let mut response = self.envelope(id);
        response["error"] = json!({"code":failure.code,"message":failure.message});
        response
    }
    fn respond(&mut self, request: &Value) -> Value {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if !(id.is_null() || id.is_string() || id.is_number()) {
            return self.error(
                Value::Null,
                fail("InvalidRequest", "id must be a string, number or null"),
            );
        }
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return self.error(id, fail("InvalidRequest", "method must be a string"));
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return self.error(id, fail("InvalidParams", "params must be an object"));
        }
        match self.execute(method, &params) {
            Ok(result) => {
                let mut response = self.envelope(id);
                response["result"] = result;
                response
            }
            Err(failure) => self.error(id, failure),
        }
    }
    fn execute(&mut self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "initialize" => {
                let mut next_workspace = self.workspace.clone();
                let mut next_entry = self.entry.clone();
                if let Some(workspace) = params.get("workspace") {
                    let next = if workspace.is_null() {
                        None
                    } else {
                        Some(
                            Path::new(workspace.as_str().ok_or_else(|| {
                                fail("InvalidParams", "workspace must be a path or null")
                            })?)
                            .canonicalize()
                            .map_err(|e| fail("InvalidPath", e.to_string()))?,
                        )
                    };
                    if let Some(path) = &next {
                        crate::input::require_package("editor", path)
                            .map_err(|message| fail("InvalidPath", message))?;
                    }
                    next_workspace = next;
                }
                if let Some(entry) = params.get("entry") {
                    next_entry = if entry.is_null() {
                        None
                    } else {
                        let path = document_path(&json!({"path":entry}))?;
                        if std::fs::metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
                            return Err(fail(
                                "InvalidPath",
                                "entry must be a regular .rinzu source file",
                            ));
                        }
                        Some(path)
                    };
                }
                if self.workspace != next_workspace || self.entry != next_entry {
                    self.workspace = next_workspace;
                    self.entry = next_entry;
                    self.cache = SourceCache::default();
                    self.generation += 1;
                }
                Ok(
                    json!({"workspace":self.workspace,"entry":self.entry,"language":rinzu_compiler::LANGUAGE_SEMANTICS_REVISION,
                    "positionEncoding":"utf-16","methods":["initialize","open","change","close","refresh","diagnostics","definition","references","completion","format","organizeImports","shutdown"],
                    "limits":{"documentBytes":DOCUMENT_BYTES,"openDocuments":OPEN_DOCUMENTS,"snapshotFiles":SNAPSHOT_FILES,"frameBytes":FRAME_BYTES,"diagnostics":DIAGNOSTICS},
                    "typechecking":"whole linked graph; unchanged module parse cache",
                    "fileEntry":"initialize.entry; .rinzu path; disk or open overlay; entry takes precedence over workspace",
                    "navigationCoverage":["functions","types","constants","locals","parameters","closure captures","public imports"],
                    "nativeValidation":"declarations only; no Clang/build/helper execution"}),
                )
            }
            "open" | "change" => {
                let path = document_path(params)?;
                let next_version = version(params)?;
                let text = string(params, "text")?;
                if text.len() > DOCUMENT_BYTES {
                    return Err(fail("LimitExceeded", "document exceeds 1 MiB"));
                }
                if text.starts_with('\u{feff}') {
                    return Err(fail(
                        "InvalidParams",
                        "BOM belongs in bom metadata, not logical text",
                    ));
                }
                let old = self.buffers.get(&path);
                if method == "open" && old.is_some() {
                    return Err(fail("AlreadyOpen", "document is already open"));
                }
                if method == "change" {
                    let old =
                        old.ok_or_else(|| fail("NotOpen", "change requires an open document"))?;
                    if next_version <= old.version {
                        return Err(fail("ContentModified", "change version must increase"));
                    }
                }
                if old.is_none() && self.buffers.len() >= OPEN_DOCUMENTS {
                    return Err(fail("LimitExceeded", "32 documents are already open"));
                }
                let bom = match params.get("bom") {
                    Some(value) => value
                        .as_bool()
                        .ok_or_else(|| fail("InvalidParams", "bom must be boolean"))?,
                    None => old.is_some_and(|b| b.bom),
                };
                self.buffers.insert(
                    path.clone(),
                    Buffer {
                        version: next_version,
                        text: text.into(),
                        bom,
                    },
                );
                self.generation += 1;
                Ok(json!({"path":path,"version":next_version}))
            }
            "close" => {
                let path = self.current(params)?;
                self.buffers.remove(&path);
                self.cache = SourceCache::default();
                self.generation += 1;
                Ok(json!({"closed":path}))
            }
            "refresh" => {
                self.cache = SourceCache::default();
                self.generation += 1;
                Ok(json!({"refreshed":true}))
            }
            "shutdown" => {
                self.buffers.clear();
                self.cache = SourceCache::default();
                self.shutdown = true;
                Ok(json!({"shutdown":true}))
            }
            "diagnostics" | "definition" | "references" | "completion" | "format"
            | "organizeImports" => {
                let path = self.current(params)?;
                let analysis = self.analyze(&path)?;
                let mut result = json!({"syntaxOnly":analysis.syntax_only,"stale":analysis.stale,
                    "inputs":analysis.receipts,"parseCache":{"hits":analysis.hits,"misses":analysis.misses}});
                if analysis.stale {
                    return Err(fail(
                        "ContentModified",
                        "disk inputs changed during analysis; refresh and retry",
                    ));
                }
                let file = analysis
                    .sources
                    .get(&path)
                    .ok_or_else(|| fail("AnalysisUnavailable", "document could not be decoded"))?;
                match method {
                    "diagnostics" => {
                        result["diagnostics"] = Value::Array(analysis.diagnostics);
                        result["omittedDiagnostics"] = json!(analysis.omitted);
                    }
                    "definition" | "references" | "completion" => {
                        let position: Utf16Position = serde_json::from_value(
                            params.get("position").cloned().unwrap_or(Value::Null),
                        )
                        .map_err(|e| fail("InvalidPosition", e.to_string()))?;
                        let byte = file
                            .original_byte_at_utf16(position)
                            .map_err(|e| fail("InvalidPosition", e))?;
                        let point = SourceRange::empty(file.id(), byte);
                        if method == "completion" {
                            let mut items: Vec<_> = analysis.index.visible_symbols(point).into_iter()
                                .map(|symbol| json!({"label":symbol.name,"kind":symbol.kind,"definition":location(symbol.definition,&analysis.sources,&analysis.paths)})).collect();
                            for keyword in [
                                "fn",
                                "async",
                                "let",
                                "var",
                                "if",
                                "else",
                                "while",
                                "for",
                                "in",
                                "return",
                                "try",
                                "catch",
                                "finally",
                                "await",
                                "spawn",
                                "class",
                                "enum",
                                "interface",
                                "Self",
                                "throws",
                                "nothrow",
                                "error",
                                "type",
                                "import",
                                "export",
                                "private",
                                "const",
                                "Int",
                                "String",
                                "Bool",
                                "Unit",
                                "List",
                                "Map",
                                "Future",
                            ] {
                                if !items.iter().any(|item| item["label"] == keyword) {
                                    items.push(json!({"label":keyword,"kind":"keyword"}));
                                }
                            }
                            result["items"] = json!(items);
                        } else if let Some(definition) = analysis.index.definition_at(point) {
                            if method == "definition" {
                                result["locations"] = json!([location(
                                    definition,
                                    &analysis.sources,
                                    &analysis.paths
                                )]);
                            } else {
                                let mut ranges = BTreeSet::new();
                                let mut references = Vec::new();
                                if params
                                    .get("includeDeclaration")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false)
                                {
                                    references.push(definition);
                                }
                                references.extend(
                                    analysis
                                        .index
                                        .references
                                        .iter()
                                        .filter(|r| r.definition == definition)
                                        .map(|r| r.range),
                                );
                                result["locations"] = json!(
                                    references
                                        .into_iter()
                                        .filter(|r| ranges.insert((
                                            r.file_id.0,
                                            r.start_byte,
                                            r.end_byte
                                        )))
                                        .map(|r| location(r, &analysis.sources, &analysis.paths))
                                        .collect::<Vec<_>>()
                                );
                            }
                        } else {
                            result["locations"] = json!([]);
                        }
                    }
                    "format" | "organizeImports" => {
                        let changed = if method == "format" {
                            rinzu_compiler::format_source(file)
                        } else {
                            organize_imports(file)
                        };
                        match changed {
                            Ok(text) => {
                                let edits = if text == file.text() {
                                    vec![]
                                } else {
                                    vec![json!({"path":path,"version":self.buffers[&path].version,
                                        "range":{"start":{"line":0,"character":0},"end":file.utf16_position(file.original_offset(file.text().len())).map_err(|e|fail("InvalidPosition",e))?},"newText":text})]
                                };
                                result["edits"] = json!(edits);
                            }
                            Err(errors) => {
                                result["edits"] = json!([]);
                                result["diagnostics"] = json!(
                                    errors
                                        .iter()
                                        .take(DIAGNOSTICS)
                                        .map(|d| diagnostic(d, &analysis.sources, &analysis.paths))
                                        .collect::<Vec<_>>()
                                );
                            }
                        }
                    }
                    _ => unreachable!(),
                }
                // Keep no typed result/index from an earlier request or version.
                Ok(result)
            }
            _ => Err(fail("MethodNotFound", format!("unknown method {method}"))),
        }
    }
    fn current(&self, params: &Value) -> Result<PathBuf> {
        let path = document_path(params)?;
        let buffer = self
            .buffers
            .get(&path)
            .ok_or_else(|| fail("NotOpen", "query requires an open document"))?;
        if version(params)? != buffer.version {
            return Err(fail("ContentModified", "query version is not current"));
        }
        Ok(path)
    }

    fn analyze(&mut self, path: &Path) -> Result<Analysis> {
        let overlays = self
            .buffers
            .iter()
            .map(|(path, buffer)| {
                let mut bytes = Vec::new();
                if buffer.bom {
                    bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
                }
                bytes.extend_from_slice(buffer.text.as_bytes());
                (path.clone(), bytes)
            })
            .collect();
        let mut snapshot = SourceSnapshot::new(&mut self.cache, overlays);
        snapshot.set_limits(DOCUMENT_BYTES + 3, SNAPSHOT_FILES);
        let mut errors = Vec::new();
        let mut linked = Vec::<Module>::new();
        let mut artifact = None;
        let mut syntax_only = false;
        if let Some(entry) = &self.entry {
            match rinzu_compiler::check_input_with_sources(
                entry,
                &rinzu_compiler::CompilationOptions {
                    language: None,
                    target: rinzu_runtime::current_target().into(),
                },
                &rinzu_compiler::NativeCliInputs::default(),
                &mut snapshot,
            ) {
                Ok(context) => {
                    artifact = Some(context.artifact);
                }
                Err(e) => {
                    errors.extend(e);
                    syntax_only = true;
                }
            }
        } else if let Some(workspace) = &self.workspace {
            match rinzu_compiler::check_package_with_sources(workspace, &mut snapshot) {
                Ok((_, a)) => {
                    artifact = Some(a);
                }
                Err(e) => {
                    errors.extend(e);
                    syntax_only = true;
                }
            }
        }
        // A loose/unreferenced buffer still gets current syntax and library checks.
        if !snapshot.sources().contains_key(path) {
            let bytes = snapshot
                .read_bytes(path)
                .map_err(|e| fail("AnalysisUnavailable", e.to_string()))?;
            let file = snapshot
                .source(path, bytes)
                .map_err(|_| fail("AnalysisUnavailable", "open buffer is not UTF-8"))?;
            match snapshot.parse(&file) {
                Ok(module) => match rinzu_compiler::check_module_with_options(
                    &module,
                    &CompilerOptions {
                        is_library: true,
                        ..CompilerOptions::default()
                    },
                ) {
                    Ok(checked) => {
                        linked.push(module);
                        linked.push(checked.syntax);
                    }
                    Err(e) => {
                        errors.extend(e);
                        syntax_only = true;
                    }
                },
                Err(e) => {
                    errors.extend(e);
                    syntax_only = true;
                }
            }
        }
        if let Some(a) = &artifact {
            linked.push(a.syntax.clone());
            linked.push(a.checked.syntax.clone());
            for package in &a.sdk_packages {
                if let Some(source) = &package.source {
                    linked.push(source.syntax.clone());
                }
            }
        }
        let sources = snapshot.sources().clone();
        let modules = snapshot.parsed_modules();
        let mut index = build_editor_index(&sources, &modules, &linked.iter().collect::<Vec<_>>());
        if let Some(a) = &artifact {
            import_symbols(&mut index, &sources, a);
        }
        let mut paths = BTreeMap::new();
        for d in &errors {
            for range in std::iter::once(&d.primary).chain(d.secondary.iter().map(|(r, _)| r)) {
                if let Some(path) = snapshot.path_for_id(range.file_id) {
                    paths.insert(range.file_id.0, path.to_owned());
                }
            }
        }
        if let Some(entry) = &self.entry {
            paths.insert(FileId::MANIFEST.0, entry.clone());
        } else if let Some(workspace) = &self.workspace
            && let Ok(path) = rinzu_compiler::find_manifest(workspace)
        {
            paths.insert(FileId::MANIFEST.0, path);
        }
        let omitted = errors.len().saturating_sub(DIAGNOSTICS);
        let diagnostics = errors
            .iter()
            .take(DIAGNOSTICS)
            .map(|d| diagnostic(d, &sources, &paths))
            .collect();
        let receipts=snapshot.input_receipts().into_iter().map(|(path,kind,bytes,hash)|json!({"path":path,"kind":kind,"bytes":bytes,"fnv1a64":format!("{hash:016x}")})).collect();
        let stale = !snapshot.disk_is_current();
        let hits = snapshot.parse_hits;
        let misses = snapshot.parse_misses;
        let mut keep = snapshot.read_paths();
        keep.extend(self.buffers.keys().cloned());
        drop(snapshot);
        self.cache.retain_paths(&keep);
        Ok(Analysis {
            sources,
            paths,
            index,
            diagnostics,
            omitted,
            receipts,
            syntax_only,
            stale,
            hits,
            misses,
        })
    }
}

struct Analysis {
    sources: BTreeMap<PathBuf, SourceFile>,
    paths: BTreeMap<u32, PathBuf>,
    index: EditorIndex,
    diagnostics: Vec<Value>,
    omitted: usize,
    receipts: Vec<Value>,
    syntax_only: bool,
    stale: bool,
    hits: usize,
    misses: usize,
}

fn location(
    range: SourceRange,
    sources: &BTreeMap<PathBuf, SourceFile>,
    paths: &BTreeMap<u32, PathBuf>,
) -> Value {
    let source = sources.values().find(|s| s.id() == range.file_id);
    let mut result = json!({"path":source.map(SourceFile::path).or_else(||paths.get(&range.file_id.0).map(PathBuf::as_path)),
        "originalBytes":{"start":range.start_byte,"end":range.end_byte}});
    if let Some(source) = source
        && let (Ok(start), Ok(end)) = (
            source.utf16_position(range.start_byte as usize),
            source.utf16_position(range.end_byte as usize),
        )
    {
        result["range"] = json!({"start":start,"end":end});
        let (line, column) = source.line_column(range.start_byte as usize);
        result["scalarStart"] = json!({"line":line,"column":column});
    }
    result["fileLevel"] = json!(result.get("range").is_none());
    result
}
fn diagnostic(
    d: &Diagnostic,
    sources: &BTreeMap<PathBuf, SourceFile>,
    paths: &BTreeMap<u32, PathBuf>,
) -> Value {
    json!({"code":d.code.as_str(),"severity":"error","message":d.message,"primary":location(d.primary,sources,paths),
        "secondary":d.secondary.iter().take(128).map(|(r,m)|json!({"location":location(*r,sources,paths),"message":m})).collect::<Vec<_>>(),
        "suggestions":d.suggestions.iter().take(128).map(|s|json!({"applicability":format!("{:?}",s.applicability),"message":s.message,
            "replacement":s.replacement.as_ref().map(|(r,text)|json!({"location":location(*r,sources,paths),"newText":text}))})).collect::<Vec<_>>(),
        "omittedSecondary":d.secondary.len().saturating_sub(128),"omittedSuggestions":d.suggestions.len().saturating_sub(128)})
}

fn import_symbols(
    index: &mut EditorIndex,
    sources: &BTreeMap<PathBuf, SourceFile>,
    artifact: &rinzu_compiler::FrontendArtifact,
) {
    for node in &artifact.module_graph {
        let Some(source) = sources.get(&node.path) else {
            continue;
        };
        let scope = SourceRange::new(
            source.id(),
            source.original_offset(0),
            source.original_offset(source.text().len()),
        );
        for import in &node.imports {
            for (public, target) in &import.public_symbols {
                index.add_import_symbol(format!("{}.{public}", import.qualifier), target, scope);
            }
        }
    }
}

fn organize_imports(file: &SourceFile) -> std::result::Result<String, Vec<Diagnostic>> {
    let module = rinzu_compiler::parse_source(file)?;
    let mut output = file.text().to_owned();
    // Only sort contiguous, comment-free import groups. Every other byte stays put.
    let mut group = Vec::new();
    let mut groups = Vec::new();
    let bom = file.original_offset(0);
    for import in &module.imports {
        if let Some(previous) = group.last().copied() {
            let previous: &rinzu_compiler::ast::Import = previous;
            let gap = &file.text()
                [previous.range.end_byte as usize - bom..import.range.start_byte as usize - bom];
            if !gap.chars().all(char::is_whitespace) {
                groups.push(std::mem::take(&mut group));
            }
        }
        group.push(import);
    }
    if !group.is_empty() {
        groups.push(group);
    }
    for group in groups.into_iter().rev() {
        if group.len() < 2 {
            continue;
        }
        let start = group[0].range.start_byte as usize - bom;
        let end = group.last().expect("import group").range.end_byte as usize - bom;
        if group.iter().any(|i| {
            let text =
                &file.text()[i.range.start_byte as usize - bom..i.range.end_byte as usize - bom];
            text.contains("//") || text.contains("/*")
        }) {
            continue;
        }
        let entries: BTreeSet<_> = group
            .iter()
            .map(|i| {
                file.text()[i.range.start_byte as usize - bom..i.range.end_byte as usize - bom]
                    .trim()
                    .to_owned()
            })
            .collect();
        output.replace_range(
            start..end,
            &entries
                .into_iter()
                .collect::<Vec<_>>()
                .join(if file.text().contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                }),
        );
    }
    rinzu_compiler::parse_source(&SourceFile::new(file.id(), file.path(), &output))?;
    Ok(output)
}
