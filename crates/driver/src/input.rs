//! Common CLI input checks and options.
use std::path::{Path, PathBuf};

use rinzu_compiler::{FrontendArtifact, Manifest, NativeCliInputs, SourceCache, SourceSnapshot};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub language: Option<String>,
    pub target: Option<String>,
}

impl Options {
    pub fn compiler(&self) -> Result<rinzu_compiler::CompilationOptions, String> {
        let host = rinzu_runtime::current_target();
        if self.target.as_deref().is_some_and(|target| target != host) {
            return Err(format!(
                "cross target is unsupported; host target is `{host}`"
            ));
        }
        Ok(rinzu_compiler::CompilationOptions {
            language: self.language.clone(),
            target: host.into(),
        })
    }
}

pub fn check(
    path: &Path,
    options: &Options,
    native: &rinzu_compiler::NativeCliInputs,
) -> Result<rinzu_compiler::CompilationContext, String> {
    let _timing = rinzu_compiler::timing::span("frontend");
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    rinzu_compiler::check_input_with_sources(path, &options.compiler()?, native, &mut sources)
        .map_err(|errors| {
            crate::cli_diagnostics::emit_snapshot(&errors, &sources);
            "input failed to check".into()
        })
}

pub fn check_package(
    path: &Path,
    native: &NativeCliInputs,
) -> Result<(Manifest, FrontendArtifact), String> {
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    let report = |errors: &[rinzu_compiler::Diagnostic], sources: &SourceSnapshot<'_>| {
        crate::cli_diagnostics::emit_snapshot(errors, sources);
        "input failed to check".to_owned()
    };
    let (mut manifest, mut artifact) =
        rinzu_compiler::check_package_with_sources(path, &mut sources)
            .map_err(|errors| report(&errors, &sources))?;
    artifact.native_inputs = native
        .normalize(&manifest)
        .map_err(|errors| report(&errors, &sources))?;
    manifest
        .native
        .headers
        .extend(artifact.native_inputs.headers.clone());
    Ok((manifest, artifact))
}

pub fn load_manifest(path: &Path) -> Result<Manifest, String> {
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    rinzu_compiler::load_manifest_with_sources(path, &mut sources).map_err(|errors| {
        crate::cli_diagnostics::emit_snapshot(&errors, &sources);
        "input failed to check".to_owned()
    })
}

pub fn require_package(command: &str, path: &Path) -> Result<(), String> {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()) {
        return Ok(());
    }
    Err(format!(
        "{command} requires a package directory; source-file mode is available for check/run/build/verify-aot"
    ))
}

pub fn protect_outputs(
    context: &rinzu_compiler::CompilationContext,
    outputs: &[PathBuf],
) -> Result<(), String> {
    let mut protected = context
        .inputs
        .iter()
        .map(|input| input.path.clone())
        .collect::<Vec<_>>();
    for ancestor in context.metadata.root.ancestors() {
        for name in [
            "rinzu.toml",
            "rinzu.lock",
            "rinzu.api.json",
            rinzu_compiler::modules_lock::FILE,
        ] {
            protected.push(ancestor.join(name));
        }
    }
    for manifest in std::iter::once(&context.metadata)
        .chain(context.artifact.sdk_packages.iter().map(|p| &p.manifest))
    {
        protected.extend(
            manifest
                .native
                .sources
                .iter()
                .chain(&manifest.native.headers)
                .chain(&manifest.native.objects)
                .map(|path| manifest.root.join(path)),
        );
        protected.push(manifest.root.join("rinzu.lock"));
    }
    protected.extend(
        context
            .artifact
            .native_inputs
            .objects
            .iter()
            .chain(&context.artifact.native_inputs.headers)
            .map(|path| context.metadata.root.join(path)),
    );
    if !context.artifact.native_inputs.libraries.is_empty() {
        // Resolve the actual selected library before any output can replace it.
        // This is a static linker/index query; it does not load user code.
        let scratch = std::env::temp_dir().join(format!(
            "rinzu-native-output-check-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        std::fs::create_dir(&scratch).map_err(|error| error.to_string())?;
        let _temporary = crate::verify_capture::TemporaryDirectory(scratch.clone());
        let index = crate::native_binding::ReferenceIndex::for_cli(
            &context.metadata.root,
            &context.artifact.native_inputs,
            &scratch,
        )?;
        protected.extend(index.link_paths);
    }
    for (index, output) in outputs.iter().enumerate() {
        let identity = if output.exists() {
            output.canonicalize().map_err(|error| error.to_string())?
        } else {
            crate::package_commands::output_identity(output)?
        };
        for input in &protected {
            let input_identity = crate::package_commands::output_identity(input)?;
            if crate::diagnostics::same_file(output, input)?
                || input_identity.starts_with(&identity)
            {
                return Err(format!("output collides with input {}", input.display()));
            }
        }
        for previous in &outputs[..index] {
            if crate::diagnostics::same_file(output, previous)? {
                return Err("outputs must be distinct".into());
            }
        }
    }
    Ok(())
}

pub fn build_native(
    context: &rinzu_compiler::CompilationContext,
) -> Result<crate::native_package::BuiltNativePackages, String> {
    crate::native_package::build_artifact(&context.metadata, &context.artifact)
}

pub fn publication_directory(
    manifest: &rinzu_compiler::Manifest,
    artifact: &rinzu_compiler::FrontendArtifact,
    scratch: &Path,
) -> PathBuf {
    if artifact.is_file_input {
        scratch.join("published-source-interfaces")
    } else {
        crate::package_tests::generated_directory(manifest)
    }
}
