//! Repository gates for Rinzu source, including generated-input templates.
use rinzu_compiler::{Diagnostic, FileId, SourceFile, format_source, parse_source};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[path = "repository_quality/check.rs"]
mod check;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("Rinzu quality: {message}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn repository_files(root: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into());
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map(PathBuf::from)
                .map_err(|error| error.to_string())
        })
        .filter(|path| {
            path.as_ref().is_err() || path.as_ref().is_ok_and(|path| root.join(path).is_file())
        })
        .collect()
}

fn is_source(path: &Path) -> bool {
    let name = path.to_string_lossy();
    name.ends_with(".rinzu") || name.ends_with(".rinzu.in")
}

fn diagnostics(path: &Path, errors: Vec<Diagnostic>) -> String {
    errors
        .iter()
        .map(|error| {
            format!(
                "{}:{}: {} {}",
                path.display(),
                error.primary.start_byte,
                error.code.as_str(),
                error.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn formatted(path: &Path, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let source = SourceFile::from_utf8(FileId(1), path, bytes.to_vec())
        .map_err(|errors| diagnostics(path, errors))?;
    let text = format_source(&source).map_err(|errors| diagnostics(path, errors))?;
    let output = SourceFile::new(FileId(1), path, &text);
    parse_source(&output).map_err(|errors| diagnostics(path, errors))?;
    if format_source(&output).map_err(|errors| diagnostics(path, errors))? != text {
        return Err(format!("formatter is not idempotent: {}", path.display()));
    }
    let mut result = Vec::new();
    if source.original_offset(0) != 0 {
        result.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    }
    result.extend_from_slice(text.as_bytes());
    Ok(result)
}

fn format_repository(root: &Path, files: &BTreeSet<PathBuf>, write: bool) -> Result<(), String> {
    // Validate the whole batch before making any writes.
    let mut changes = Vec::new();
    let mut count = 0;
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    for path in files.iter().filter(|path| is_source(path)) {
        let canonical = root
            .join(path)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !canonical.starts_with(&canonical_root) {
            return Err(format!(
                "source symlink leaves repository: {}",
                path.display()
            ));
        }
        let original = fs::read(root.join(path)).map_err(|error| error.to_string())?;
        let output = formatted(path, &original)?;
        if output != original {
            if write
                && fs::metadata(&canonical)
                    .map_err(|error| error.to_string())?
                    .permissions()
                    .readonly()
            {
                return Err(format!(
                    "refusing to format read-only source {}",
                    path.display()
                ));
            }
            changes.push((path, canonical, original, output));
        }
        count += 1;
    }
    if count == 0 {
        return Err("no repository Rinzu sources".into());
    }
    for (_, path, original, _) in &changes {
        if write && fs::read(path).map_err(|error| error.to_string())? != *original {
            return Err(format!(
                "source changed while formatting: {}",
                path.display()
            ));
        }
    }
    for (path, canonical, original, output) in &changes {
        if write {
            write_formatted(canonical, original, output)?;
        } else {
            eprintln!("would format {}", path.display());
        }
    }
    if !write && !changes.is_empty() {
        return Err(format!(
            "{} of {count} sources need formatting",
            changes.len()
        ));
    }
    println!(
        "Rinzu format {}: {count} sources ({} changed)",
        if write { "written" } else { "passed" },
        changes.len()
    );
    Ok(())
}

fn write_formatted(path: &Path, original: &[u8], output: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if fs::read(path).map_err(|error| error.to_string())? != original {
        return Err(format!(
            "source changed while formatting: {}",
            path.display()
        ));
    }
    let permissions = fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    if permissions.readonly() {
        return Err(format!(
            "refusing to format read-only source {}",
            path.display()
        ));
    }
    let temporary = path.with_file_name(format!(
        ".{}.rinzu-fmt-{}-{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let temporary = Temporary(temporary);
    file.write_all(output).map_err(|error| error.to_string())?;
    file.set_permissions(permissions)
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temporary.0, path).map_err(|error| error.to_string())
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let root = root();
    let mut files = repository_files(&root)?;
    if arguments.len() > 1 {
        let mut selected = BTreeSet::new();
        for argument in &arguments[1..] {
            let absolute = root
                .join(argument)
                .canonicalize()
                .map_err(|error| format!("{argument}: {error}"))?;
            let path = absolute
                .strip_prefix(&root)
                .map_err(|_| format!("path outside repository: {argument}"))?;
            let matching: Vec<_> = files
                .iter()
                .filter(|file| file.starts_with(path))
                .cloned()
                .collect();
            if !matching.iter().any(|path| is_source(path)) {
                return Err(format!("no repository Rinzu source at {argument}"));
            }
            selected.extend(matching);
            for parent in absolute.ancestors().skip(1) {
                if !parent.starts_with(&root) {
                    break;
                }
                let manifest = parent.join("rinzu.toml");
                if manifest.is_file() {
                    selected.insert(manifest.strip_prefix(&root).unwrap().to_owned());
                    break;
                }
            }
        }
        files = selected;
    }
    match arguments.first().map(String::as_str) {
        Some("fmt") => format_repository(&root, &files, false),
        Some("fmt-write") => format_repository(&root, &files, true),
        Some("lint") => check::repository(&root, &files),
        _ => Err("usage: cargo run -p rinzu-compiler --example repository_quality -- <fmt|fmt-write|lint> [paths...]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Repository(PathBuf);
    impl Repository {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "rinzu-quality-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            assert!(
                Command::new("git")
                    .args(["init", "--quiet"])
                    .current_dir(&path)
                    .status()
                    .unwrap()
                    .success()
            );
            Self(path)
        }
        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }
    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn inventory_includes_new_nested_sources_and_templates_but_honors_gitignore() {
        let repo = Repository::new();
        repo.write(".gitignore", "ignored/\n");
        repo.write("src/main.rinzu", "fn main() {}\n");
        assert!(
            Command::new("git")
                .args(["add", "src/main.rinzu"])
                .current_dir(&repo.0)
                .status()
                .unwrap()
                .success()
        );
        repo.write(".fixtures/new/template.rinzu.in", "fn main() {}\n");
        repo.write("new/test.rinzu", "fn main() {}\n");
        repo.write("ignored/bad.rinzu", "invalid");
        let files = repository_files(&repo.0).unwrap();
        let sources: Vec<_> = files
            .iter()
            .filter(|path| is_source(path))
            .map(|path| path.to_str().unwrap())
            .collect();
        assert_eq!(
            sources,
            [
                ".fixtures/new/template.rinzu.in",
                "new/test.rinzu",
                "src/main.rinzu"
            ]
        );
    }

    #[test]
    fn concurrent_edits_are_preserved_instead_of_overwritten() {
        let repo = Repository::new();
        repo.write("main.rinzu", "fn main(){let user=1;}");
        assert!(
            write_formatted(
                &repo.0.join("main.rinzu"),
                b"fn main(){}",
                b"fn main() {}\n"
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(repo.0.join("main.rinzu")).unwrap(),
            "fn main(){let user=1;}"
        );
    }

    #[test]
    fn invalid_later_source_prevents_all_format_writes() {
        let repo = Repository::new();
        repo.write("a.rinzu", "fn main(){if ! true{}}");
        repo.write("z.rinzu.in", "@UNKNOWN@");
        let files = repository_files(&repo.0).unwrap();
        assert!(format_repository(&repo.0, &files, true).is_err());
        assert_eq!(
            fs::read_to_string(repo.0.join("a.rinzu")).unwrap(),
            "fn main(){if ! true{}}"
        );
    }
}
