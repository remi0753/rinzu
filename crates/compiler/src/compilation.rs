//! Explicit source-file or directory-package selection, without generated configuration.
use std::path::{Path, PathBuf};

use crate::{
    Diagnostic, DiagnosticCode, FileId, FrontendArtifact, Manifest, NativeCliInputs,
    NativeManifest, PackageExports, PackageKind, SourceCache, SourceRange, SourceSnapshot,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum CompilationMode {
    File,
    Package,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationOptions {
    pub language: Option<String>,
    /// Backend target selected by the embedding driver, never inferred from a nearby manifest.
    pub target: String,
}

#[derive(Clone, Debug)]
pub struct CompilationContext {
    pub mode: CompilationMode,
    pub entry: PathBuf,
    /// Compatibility metadata for existing runtime APIs; file mode never loads/saves a manifest.
    pub metadata: Manifest,
    pub target: String,
    pub cwd: PathBuf,
    pub artifact: FrontendArtifact,
    pub inputs: Vec<CompilationInput>,
    pub options: CompilationOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilationInput {
    pub path: PathBuf,
    pub kind: String,
    pub byte_length: usize,
    pub digest: u64,
}

impl CompilationContext {
    #[must_use]
    pub fn default_output(&self) -> PathBuf {
        match self.mode {
            CompilationMode::File => self.cwd.join("build").join(&self.metadata.name),
            CompilationMode::Package => self.metadata.root.join("build").join(&self.metadata.name),
        }
    }
}

pub fn check_input(
    start: &Path,
    options: &CompilationOptions,
    native: &NativeCliInputs,
) -> Result<CompilationContext, Vec<Diagnostic>> {
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    check_input_with_sources(start, options, native, &mut sources)
}

pub fn check_input_with_sources(
    start: &Path,
    options: &CompilationOptions,
    native: &NativeCliInputs,
    sources: &mut SourceSnapshot<'_>,
) -> Result<CompilationContext, Vec<Diagnostic>> {
    let cwd = std::env::current_dir().map_err(|error| input_error(start, error.to_string()))?;
    let absolute = if start.is_absolute() {
        start.to_owned()
    } else {
        cwd.join(start)
    };
    let (stat, canonical) = match std::fs::metadata(&absolute) {
        Ok(stat) => {
            let canonical = sources.canonical_path(&absolute).map_err(|error| {
                input_error(&absolute, format!("cannot resolve input: {error}"))
            })?;
            (Some(stat), canonical)
        }
        Err(error) => {
            let canonical = sources
                .canonical_path(&absolute)
                .ok()
                .filter(|path| sources.has_overlay(path));
            if error.kind() != std::io::ErrorKind::NotFound || canonical.is_none() {
                return Err(input_error(
                    &absolute,
                    format!("cannot inspect input: {error}"),
                ));
            }
            (None, canonical.unwrap())
        }
    };
    if stat
        .as_ref()
        .is_some_and(|stat| !stat.is_dir() && !stat.is_file())
    {
        return Err(input_error(
            &absolute,
            "input must be a regular source file or directory",
        ));
    }
    let (mode, metadata, mut artifact, entry) = if stat
        .as_ref()
        .is_some_and(std::fs::Metadata::is_dir)
    {
        if options.language.is_some() {
            return Err(input_error(
                &canonical,
                "--language cannot override package language; edit the manifest or specify a source file",
            ));
        }
        let (mut manifest, mut artifact) = crate::check_package_with_sources(&canonical, sources)?;
        artifact.native_inputs = native.normalize(&manifest)?;
        manifest
            .native
            .headers
            .extend(artifact.native_inputs.headers.clone());
        let entry = sources
            .canonical_path(&manifest.root.join(&manifest.entry))
            .map_err(|error| {
                input_error(&canonical, format!("cannot resolve package entry: {error}"))
            })?;
        (CompilationMode::Package, manifest, artifact, entry)
    } else {
        if absolute
            .file_name()
            .is_some_and(|name| name == "rinzu.toml")
            || canonical
                .file_name()
                .is_some_and(|name| name == "rinzu.toml")
        {
            return Err(input_error(
                &absolute,
                "rinzu.toml is configuration; pass its package directory",
            ));
        }
        if options
            .language
            .as_deref()
            .is_some_and(|version| version != "0.5")
        {
            return Err(input_error(
                &absolute,
                "file mode supports --language 0.5 only",
            ));
        }
        let mut artifact = crate::package::check_file_with_sources(&canonical, sources)?;
        let name = canonical
            .file_stem()
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "program".into());
        let mut metadata = Manifest {
            root: canonical
                .parent()
                .expect("canonical file has a parent")
                .to_owned(),
            name,
            language: "0.5".into(),
            kind: PackageKind::Application,
            entry: PathBuf::from(canonical.file_name().expect("canonical file has a name")),
            dependencies: Vec::new(),
            exports: PackageExports::default(),
            native: NativeManifest::default(),
        };
        let mut cli_owner = metadata.clone();
        cli_owner.root = cwd.clone();
        artifact.native_inputs = native.normalize(&cli_owner)?;
        metadata
            .native
            .headers
            .extend(artifact.native_inputs.headers.clone());
        (CompilationMode::File, metadata, artifact, canonical)
    };
    if options.target != crate::standard_library::TARGET
        && artifact
            .sdk_packages
            .iter()
            .any(|p| crate::standard_library::SOURCE_PACKAGES.contains(&p.manifest.name.as_str()))
    {
        return Err(input_error(
            &entry,
            format!(
                "standard library supports target {}; requested {}",
                crate::standard_library::TARGET,
                options.target
            ),
        ));
    }
    for header in &metadata.native.headers {
        let path = metadata.root.join(header);
        sources
            .read_native_header(&path)
            .map_err(|error| input_error(&path, format!("cannot read native header: {error}")))?;
    }
    artifact.checked_inputs = sources.input_bytes();
    Ok(CompilationContext {
        mode,
        entry,
        metadata,
        target: options.target.clone(),
        cwd,
        artifact,
        inputs: sources
            .input_receipts()
            .into_iter()
            .map(|(path, kind, byte_length, digest)| CompilationInput {
                path,
                kind: kind.into(),
                byte_length,
                digest,
            })
            .collect(),
        options: options.clone(),
    })
}

fn input_error(path: &Path, message: impl Into<String>) -> Vec<Diagnostic> {
    vec![
        Diagnostic::error(
            DiagnosticCode::PackageIo,
            SourceRange::empty(FileId::MANIFEST, 0),
            message,
            "pass a readable source file or package directory explicitly",
        )
        .with_path(path),
    ]
}
