use crate::{Diagnostic, DiagnosticCode, FileId, Manifest, SourceRange};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Explicit application build-graph additions, before or after root normalization.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCliInputs {
    pub search_paths: Vec<PathBuf>,
    pub libraries: Vec<String>,
    pub objects: Vec<PathBuf>,
    /// Optional physical prototypes; these never grant symbol references.
    #[serde(default)]
    pub headers: Vec<PathBuf>,
}

impl NativeCliInputs {
    pub fn is_empty(&self) -> bool {
        self.search_paths.is_empty()
            && self.libraries.is_empty()
            && self.objects.is_empty()
            && self.headers.is_empty()
    }
    pub fn normalize(&self, manifest: &Manifest) -> Result<Self, Vec<Diagnostic>> {
        if self.libraries.iter().any(|name| {
            name.is_empty()
                || name.starts_with('-')
                || !name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "_+.-".contains(ch))
        }) {
            return Err(vec![Diagnostic::error(
                DiagnosticCode::ManifestInvalid,
                SourceRange::empty(FileId::MANIFEST, 0),
                "invalid CLI native library name",
                "pass one explicit library name without path or flag syntax",
            )]);
        }
        let mut libraries = BTreeSet::new();
        Ok(Self {
            objects: normalize_paths(&manifest.root, &self.objects, "object", false)?,
            search_paths: normalize_paths(&manifest.root, &self.search_paths, "search path", true)?,
            headers: normalize_paths(&manifest.root, &self.headers, "header", false)?,
            libraries: self
                .libraries
                .iter()
                .filter(|name| libraries.insert((*name).clone()))
                .cloned()
                .collect(),
        })
    }
}

fn normalize_paths(
    root: &Path,
    paths: &[PathBuf],
    kind: &str,
    directory: bool,
) -> Result<Vec<PathBuf>, Vec<Diagnostic>> {
    let error = |path: &Path, message: String| {
        vec![Diagnostic::error(
            DiagnosticCode::ManifestInvalid,
            SourceRange::empty(FileId::MANIFEST, 0),
            message,
            "pass an existing regular native file or search directory; CLI paths may be absolute",
        ).with_path(path)]
    };
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for path in paths {
        if path.as_os_str().is_empty() {
            return Err(error(root, format!("native {kind} path is empty")));
        }
        let full = root.join(path);
        let canonical = full
            .canonicalize()
            .map_err(|failure| error(&full, format!("cannot resolve native {kind}: {failure}")))?;
        let metadata = canonical.metadata().map_err(|failure| {
            error(
                &canonical,
                format!("cannot inspect native {kind}: {failure}"),
            )
        })?;
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(error(
                &canonical,
                format!(
                    "native {kind} has the wrong file kind (requires {})",
                    if directory {
                        "directory"
                    } else {
                        "regular file"
                    },
                ),
            ));
        }
        if kind == "object"
            && !matches!(
                canonical.extension().and_then(|value| value.to_str()),
                Some("o" | "a" | "dylib" | "so" | "dll" | "lib")
            )
        {
            return Err(error(
                &canonical,
                "unsupported native object extension".into(),
            ));
        }
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            format!("{}:{}", metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = canonical.to_string_lossy().into_owned();
        if seen.insert(identity) {
            output.push(canonical);
        }
    }
    Ok(output)
}
