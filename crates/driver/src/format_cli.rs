//! Public formatting entry point. Parsing/formatting stays in the compiler.
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rinzu_compiler::{FileId, SourceFile, format_source};

use crate::cli_diagnostics::{self, Input as DiagnosticInput, Renderer};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode {
    Stdout,
    Check,
    Write,
}

struct Input {
    path: PathBuf,
    original: Vec<u8>,
    formatted: Vec<u8>,
    permissions: fs::Permissions,
}

/// Returns true only when a read-only check found formatting differences.
pub fn run(arguments: impl Iterator<Item = String>) -> Result<bool, String> {
    let mut mode = Mode::Stdout;
    let mut paths = Vec::new();
    let mut positional = false;
    for argument in arguments {
        match argument.as_str() {
            "--" if !positional => positional = true,
            "--check" | "--write" if !positional => {
                if mode != Mode::Stdout {
                    return Err("fmt accepts one of --check or --write".into());
                }
                mode = if argument == "--check" {
                    Mode::Check
                } else {
                    Mode::Write
                };
            }
            "--help" | "-h" if !positional => {
                println!(
                    "rinzu fmt <file|directory>... [--check | --write]\nrinzu fmt - [--check]\nWithout a mode, one file or stdin is formatted to stdout.\nDirectories require --check or --write; hidden/build/target directories and discovered symlinks are skipped."
                );
                return Ok(false);
            }
            "-" => paths.push(PathBuf::from("-")),
            other if other.starts_with('-') && !positional => {
                return Err(format!(
                    "unknown fmt option `{other}`; use -- before a path starting with -"
                ));
            }
            _ => paths.push(PathBuf::from(argument)),
        }
    }
    if paths.is_empty() {
        return Err("fmt requires a source path or - for stdin; see rinzu fmt --help".into());
    }
    if paths.iter().any(|path| path == Path::new("-")) {
        if paths.len() != 1 || mode == Mode::Write {
            return Err("fmt stdin requires a single - and does not support --write".into());
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|error| format!("read fmt stdin: {error}"))?;
        let formatted = format_bytes(Path::new("<stdin>"), &bytes)?;
        if mode == Mode::Check {
            let changed = bytes != formatted;
            if changed {
                eprintln!("would format <stdin>");
            }
            return Ok(changed);
        }
        std::io::stdout()
            .write_all(&formatted)
            .map_err(|error| format!("write formatted stdout: {error}"))?;
        return Ok(false);
    }
    if mode == Mode::Stdout && (paths.len() != 1 || paths[0].is_dir()) {
        return Err(
            "fmt stdout requires one file; use --check or --write for directories/multiple files"
                .into(),
        );
    }
    let mut files = BTreeSet::new();
    for path in paths {
        collect(&path, &mut files)?;
    }
    if files.is_empty() {
        return Err("fmt found no .rinzu source files".into());
    }
    // Parse the whole batch before the first file write. A syntax/UTF-8 error
    // in a later file must not leave earlier valid sources partly formatted.
    let inputs = files
        .into_iter()
        .map(|path| {
            let original = fs::read(&path).map_err(|error| {
                cli_diagnostics::file_failure(
                    &path,
                    format!("cannot read source: {error}"),
                    "make the source file readable",
                )
            })?;
            let formatted = format_bytes(&path, &original)?;
            let permissions = fs::metadata(&path)
                .map_err(|error| format!("inspect {}: {error}", path.display()))?
                .permissions();
            if mode == Mode::Write && original != formatted && permissions.readonly() {
                return Err(format!(
                    "refusing to format read-only source {}",
                    path.display()
                ));
            }
            Ok(Input {
                path,
                original,
                formatted,
                permissions,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut changed = false;
    for input in inputs {
        match mode {
            Mode::Stdout => std::io::stdout()
                .write_all(&input.formatted)
                .map_err(|error| format!("write formatted stdout: {error}"))?,
            Mode::Check if input.original != input.formatted => {
                changed = true;
                eprintln!("would format {}", input.path.display());
            }
            Mode::Write if input.original != input.formatted => {
                replace(&input)?;
                eprintln!("formatted {}", input.path.display());
            }
            _ => {}
        }
    }
    Ok(changed)
}

fn collect(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| {
        cli_diagnostics::file_failure(
            path,
            format!("cannot inspect source: {error}"),
            "pass a readable source file or directory",
        )
    })?;
    if metadata.is_file() {
        if path.extension().is_none_or(|value| value != "rinzu") {
            return Err(format!(
                "fmt source must have .rinzu extension: {}",
                path.display()
            ));
        }
        files.insert(
            path.canonicalize()
                .map_err(|error| format!("resolve {}: {error}", path.display()))?,
        );
    } else if metadata.is_dir() {
        for entry in fs::read_dir(path)
            .map_err(|error| format!("read directory {}: {error}", path.display()))?
        {
            let entry = entry.map_err(|error| format!("read source entry: {error}"))?;
            let kind = entry
                .file_type()
                .map_err(|error| format!("inspect source entry: {error}"))?;
            let child = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with('.') && !matches!(name.as_ref(), "build" | "target") {
                    collect(&child, files)?;
                }
            } else if kind.is_file() && child.extension().is_some_and(|value| value == "rinzu") {
                collect(&child, files)?;
            }
        }
    } else {
        return Err(format!(
            "fmt input is not a regular source or directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn format_bytes(path: &Path, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let id = FileId(1);
    let source = SourceFile::from_utf8(id, path, bytes.to_vec()).map_err(|errors| {
        cli_diagnostics::emit(&errors, |file| {
            (file == id).then_some(DiagnosticInput::Bytes(path, Some(bytes)))
        });
        "input failed to format".to_owned()
    })?;
    let text = format_source(&source).map_err(|errors| {
        cli_diagnostics::emit_file(&errors, &source);
        "input failed to format".to_owned()
    })?;
    // Refuse to publish invalid output if a formatter regression is introduced.
    let generated = format!("<formatted {}>", Renderer::new().path(path));
    let formatted = SourceFile::new(id, generated, text);
    rinzu_compiler::parse_source(&formatted).map_err(|errors| {
        cli_diagnostics::emit_file(&errors, &formatted);
        "formatter produced invalid output".to_owned()
    })?;
    let text = formatted.text();
    let mut result = Vec::with_capacity(text.len() + source.original_offset(0));
    if source.original_offset(0) != 0 {
        result.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    result.extend_from_slice(text.as_bytes());
    Ok(result)
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn replace(input: &Input) -> Result<(), String> {
    if fs::read(&input.path)
        .map_err(|error| format!("re-read {}: {error}", input.path.display()))?
        != input.original
    {
        return Err(format!(
            "source changed during formatting: {}",
            input.path.display()
        ));
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary_path = input.path.with_file_name(format!(
        ".rinzu-fmt-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|error| format!("prepare formatted {}: {error}", input.path.display()))?;
    let temporary = Temporary(temporary_path);
    file.set_permissions(input.permissions.clone())
        .map_err(|error| format!("preserve source permissions: {error}"))?;
    file.write_all(&input.formatted)
        .map_err(|error| format!("write formatted {}: {error}", input.path.display()))?;
    file.sync_all()
        .map_err(|error| format!("flush formatted {}: {error}", input.path.display()))?;
    fs::rename(&temporary.0, &input.path)
        .map_err(|error| format!("replace formatted {}: {error}", input.path.display()))?;
    Ok(())
}
