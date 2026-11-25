use std::fs;
use std::path::{Path, PathBuf};

use rinzu_bindgen::source_package::sha256;
use serde::{Deserialize, Serialize};

pub(super) struct Selection {
    pub root: PathBuf,
    pub scope: PathBuf,
    tests: PathBuf,
}

pub(super) struct FileEntry {
    pub path: PathBuf,
    pub relative: String,
}

impl FileEntry {
    pub fn identity(&self) -> String {
        format!("file:{}", self.relative)
    }
}

impl Selection {
    pub fn from_path(path: &Path) -> Result<Option<Self>, String> {
        let requested = std::path::absolute(path).map_err(|error| error.to_string())?;
        let scope = requested
            .canonicalize()
            .map_err(|error| format!("{}: {error}", requested.display()))?;
        let outside = || {
            format!(
                "{} resolves outside the package tests directory",
                requested.display()
            )
        };
        let tests_root = |root: &Path| {
            rinzu_compiler::test_source_root(root)
                .map_err(|error| format!("{}: {error}", root.join("tests").display()))
        };
        let requested_manifest = rinzu_compiler::find_manifest(&requested).ok();
        let requested_package = requested_manifest
            .as_deref()
            .and_then(Path::parent)
            .filter(|root| requested.starts_with(root.join("tests")));
        if let Some(root) = requested_package
            && !scope.starts_with(tests_root(root)?)
        {
            return Err(outside());
        }

        // A test alias may point into a nested package. Use the same owner and
        // relative ID as an invocation with the physical path.
        if let Ok(manifest) = rinzu_compiler::find_manifest(&scope) {
            let root = manifest.parent().expect("manifest has a parent");
            let tests = if scope.starts_with(root.join("tests")) {
                Some(tests_root(root)?)
            } else {
                // The package may use a symlink as its tests directory.
                rinzu_compiler::test_source_root(root).ok()
            };
            if let Some(tests) = tests.filter(|tests| scope.starts_with(tests)) {
                return Ok(Some(Self {
                    root: root.to_path_buf(),
                    scope,
                    tests,
                }));
            }
        }
        if requested_package.is_some() {
            return Err(outside());
        }
        Ok(None)
    }

    pub fn entries(&self) -> Result<Vec<FileEntry>, String> {
        let entry = |path: PathBuf| {
            let relative = rinzu_compiler::test_file_path(&self.tests, &path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            Ok::<_, String>(FileEntry { path, relative })
        };
        if !self.scope.is_dir() {
            return Ok(vec![entry(self.scope.clone())?]);
        }
        let mut entries = Vec::new();
        let mut pending = vec![self.scope.clone()];
        while let Some(directory) = pending.pop() {
            for child in fs::read_dir(&directory)
                .map_err(|error| format!("{}: {error}", directory.display()))?
            {
                let child = child.map_err(|error| format!("{}: {error}", directory.display()))?;
                let kind = child
                    .file_type()
                    .map_err(|error| format!("{}: {error}", child.path().display()))?;
                if kind.is_dir() {
                    let path = child.path();
                    if !path.join("rinzu.toml").is_file() {
                        pending.push(path);
                    }
                } else if kind.is_file()
                    && child
                        .file_name()
                        .as_encoded_bytes()
                        .ends_with(b".test.rinzu")
                {
                    entries.push(entry(child.path())?);
                }
            }
        }
        entries.sort_by(|left, right| left.relative.cmp(&right.relative));
        Ok(entries)
    }
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
pub(super) struct Inputs {
    manifest: String,
    modules_lock: Option<String>,
}

impl Inputs {
    pub fn read(root: &Path) -> Result<Self, String> {
        let manifest = root.join("rinzu.toml");
        let bytes =
            fs::read(&manifest).map_err(|error| format!("{}: {error}", manifest.display()))?;
        let lock = root.join("rinzu.modules.lock");
        let modules_lock = match fs::read(&lock) {
            Ok(bytes) => Some(sha256(&bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("{}: {error}", lock.display())),
        };
        Ok(Self {
            manifest: sha256(&bytes),
            modules_lock,
        })
    }
}
