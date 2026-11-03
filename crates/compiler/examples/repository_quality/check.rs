use super::{diagnostics, is_source};
use rinzu_compiler::{
    CompilationOptions, CompilerOptions, Diagnostic, FileId, FrontendArtifact, NativeCliInputs,
    SourceCache, SourceFile, SourceSnapshot, check_input, check_input_with_sources,
    compile_source_with_options,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "fixtures.rs"]
mod fixtures;

pub fn options() -> CompilationOptions {
    CompilationOptions {
        language: None,
        target: rinzu_compiler::standard_library::TARGET.into(),
    }
}

pub fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

pub fn source(path: &Path, text: String) -> SourceFile {
    SourceFile::new(FileId(1), path, text)
}

pub fn check_overlays(
    entry: &Path,
    overlays: BTreeMap<PathBuf, Vec<u8>>,
) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    let mut cache = SourceCache::default();
    let mut snapshot = SourceSnapshot::new(&mut cache, overlays);
    check_input_with_sources(
        entry,
        &options(),
        &NativeCliInputs::default(),
        &mut snapshot,
    )
    .map(|context| context.artifact)
}

/// A module needs a Main supplied by its importer, rather than relaxed entry
/// checks on the module itself. All its declarations still pass typed IR checks.
fn library(path: &Path) -> Result<FrontendArtifact, Vec<Diagnostic>> {
    let entry = path.parent().unwrap().join("rinzu-quality-entry.rinzu");
    let text = format!(
        "import {} as qualityModule;\nfn main() {{}}\n",
        serde_json::to_string(&path.to_string_lossy()).unwrap()
    );
    check_overlays(&entry, [(entry.clone(), text.into_bytes())].into())
}

fn diagnostic_lines(errors: &[Diagnostic]) -> String {
    errors
        .iter()
        .map(|error| {
            format!(
                "{}|{}",
                error.code.as_str(),
                error.message.split("; import chain:").next().unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn expected_failure(
    root: &Path,
    path: &Path,
    text: &str,
    _temporary: &Path,
) -> Option<Result<(), String>> {
    let golden = path.with_extension("golden");
    if !path.starts_with("crates/compiler/tests/fixtures/invalid") {
        return None;
    }
    eprintln!("Rinzu check negative fixture: {}", path.display());
    Some((|| {
        let expected = read(&root.join(golden))?;
        let file = source(path, text.to_owned());
        let errors = compile_source_with_options(&file, &CompilerOptions::default())
            .err()
            .ok_or_else(|| format!("negative fixture unexpectedly passed: {}", path.display()))?;
        let actual = diagnostic_lines(&errors);
        if actual != expected {
            return Err(format!(
                "{}: expected diagnostics:\n{expected}actual:\n{actual}",
                path.display()
            ));
        }
        Ok(())
    })())
}

pub fn repository(root: &Path, files: &BTreeSet<PathBuf>) -> Result<(), String> {
    let mut covered = BTreeMap::new();
    let mut failures = Vec::new();
    let mut packages = 0;
    for path in files.iter().filter(|path| {
        path.file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("rinzu.toml"))
    }) {
        eprintln!("Rinzu check package: {}", path.display());
        match check_input(
            root.join(path).parent().unwrap(),
            &options(),
            &NativeCliInputs::default(),
        ) {
            Ok(context) => {
                cover(
                    root,
                    &context.artifact,
                    &format!("package {}", path.display()),
                    &mut covered,
                );
                for input in context.artifact.checked_inputs.keys() {
                    if let Ok(input) = input.strip_prefix(root) {
                        covered.insert(input.to_owned(), format!("package {}", path.display()));
                    }
                }
            }
            Err(errors) => record_failure(&mut failures, diagnostics(path, errors)),
        }
        packages += 1;
    }
    let temporary = Temporary(
        std::env::temp_dir().join(format!("rinzu-repository-quality-{}", std::process::id())),
    );
    fs::create_dir_all(&temporary.0).map_err(|error| error.to_string())?;
    for path in files.iter().filter(|path| is_source(path)) {
        let absolute = root.join(path);
        let text = read(&absolute)?;
        if let Some(result) = expected_failure(root, path, &text, &temporary.0) {
            match result {
                Ok(()) => {
                    covered.insert(path.clone(), "expected diagnostics".into());
                }
                Err(error) => record_failure(&mut failures, error),
            }
            continue;
        }
        if covered.contains_key(path) {
            continue;
        }
        eprintln!("Rinzu check source: {}", path.display());
        if let Some(result) = fixtures::check(root, path, &text, &temporary.0) {
            match result {
                Ok(()) => {
                    covered.insert(path.clone(), "fixture context".into());
                }
                Err(error) => record_failure(&mut failures, error),
            }
            continue;
        }
        let result = check_input(&absolute, &options(), &NativeCliInputs::default())
            .map(|context| context.artifact);
        let result = match result {
            Err(errors) if errors.iter().all(|error| error.code.as_str() == "RZ0404") => {
                library(&absolute)
            }
            other => other,
        };
        match result {
            Ok(artifact) => {
                cover(
                    root,
                    &artifact,
                    &format!("entry {}", path.display()),
                    &mut covered,
                );
                for input in artifact.checked_inputs.keys() {
                    if let Ok(input) = input.strip_prefix(root) {
                        covered.insert(input.to_owned(), format!("entry {}", path.display()));
                    }
                }
                covered.insert(path.clone(), "entry".into());
            }
            Err(errors) => record_failure(&mut failures, diagnostics(path, errors)),
        }
    }
    let sources: BTreeMap<_, _> = files
        .iter()
        .filter(|path| is_source(path))
        .map(|path| {
            (
                path,
                covered.get(path).map(String::as_str).unwrap_or("FAILED"),
            )
        })
        .collect();
    let report = root.join("target/rinzu-quality.json");
    fs::create_dir_all(report.parent().unwrap()).map_err(|error| error.to_string())?;
    fs::write(&report, serde_json::to_vec_pretty(&sources).unwrap())
        .map_err(|error| error.to_string())?;
    if !failures.is_empty() {
        return Err(format!(
            "{} checks failed; see diagnostics above and {}",
            failures.len(),
            report.display()
        ));
    }
    println!(
        "Rinzu lint passed: {} sources, {packages} packages; coverage: {}",
        sources.len(),
        report.display()
    );
    Ok(())
}

fn record_failure(failures: &mut Vec<String>, message: String) {
    eprintln!("{message}");
    failures.push(message);
}

fn cover(
    root: &Path,
    artifact: &FrontendArtifact,
    context: &str,
    covered: &mut BTreeMap<PathBuf, String>,
) {
    for module in &artifact.module_graph {
        if let Ok(input) = module.path.strip_prefix(root) {
            covered.insert(input.to_owned(), context.to_owned());
        }
    }
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
