//! Common CLI input checks and options.
use std::path::{Path, PathBuf};

use rinzu_abi::target::Target;
use rinzu_compiler::{
    FrontendArtifact, Manifest, NativeCliInputs, SdkCheckProgress, SourceCache, SourceSnapshot,
};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub language: Option<String>,
    pub target: Option<String>,
}

impl Options {
    pub fn compiler(&self) -> Result<rinzu_compiler::CompilationOptions, String> {
        let host = rinzu_runtime::current_target();
        let requested = self.target.as_deref().unwrap_or(host);
        let linux_alias = Target::host() == Some(Target::X86_64UnknownLinuxGnu)
            && requested == "x86_64-unknown-linux";
        if requested != host && !linux_alias {
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

pub(crate) struct CheckedRun {
    pub context: rinzu_compiler::CompilationContext,
    pub dependencies: Option<crate::native_package::PreparedDependencies>,
    pub pending: Option<crate::frontend_run_cache::Pending>,
    pub generation: Option<String>,
}

pub(crate) fn check_for_run(
    path: &Path,
    options: &Options,
    native: &NativeCliInputs,
) -> Result<CheckedRun, String> {
    let mut cache = SourceCache::default();
    check_cached(path, options, native, &mut cache, true, None)
}

pub(crate) fn check_for_build(
    path: &Path,
    options: &Options,
    native: &NativeCliInputs,
) -> Result<rinzu_compiler::CompilationContext, String> {
    let mut cache = SourceCache::default();
    let checked = check_cached(path, options, native, &mut cache, false, None)?;
    // Cache the checked frontend. Each build still prepares and verifies
    // its native inputs independently of the frontend cache.
    if let Some(pending) = checked.pending {
        pending.store(&checked.context);
    }
    Ok(checked.context)
}

/// A preflight publishes only a fresh, checked frontend. Each child still
/// validates its own inputs, native images and executable code before running.
pub(crate) fn check_for_dev(
    path: &Path,
    cache: &mut SourceCache,
    paths: &mut std::collections::BTreeSet<PathBuf>,
) -> Result<rinzu_compiler::CompilationContext, String> {
    let _timing = rinzu_compiler::timing::span("dev.preflight");
    let checked = check_cached(
        path,
        &Options::default(),
        &NativeCliInputs::default(),
        cache,
        false,
        Some(paths),
    )?;
    cache.retain_paths(paths);
    if let Some(pending) = checked.pending {
        pending.store(&checked.context);
    }
    Ok(checked.context)
}

fn check_cached(
    path: &Path,
    options: &Options,
    native: &NativeCliInputs,
    cache: &mut SourceCache,
    prepare_dependencies: bool,
    watch: Option<&mut std::collections::BTreeSet<PathBuf>>,
) -> Result<CheckedRun, String> {
    let options = options.compiler()?;
    crate::jit_progress::phase("Checking frontend cache");
    let entry = crate::frontend_run_cache::Cache::open(path, &options, native);
    if let Some((context, generation)) = entry
        .as_ref()
        .and_then(|entry| entry.restore(&options, native))
    {
        if let Some(paths) = watch {
            paths.extend(context.inputs.iter().map(|input| input.path.clone()));
        }
        return Ok(CheckedRun {
            context,
            dependencies: None,
            pending: None,
            generation: Some(generation),
        });
    }

    let mut sdk_cache = entry.as_ref().and_then(|entry| entry.sdk_cache());
    let mut functions = entry.as_ref().and_then(|entry| entry.function_cache());
    let mut progress = crate::jit_progress::frontend_progress;
    let mut sources = SourceSnapshot::new(cache, Default::default());
    sources.set_progress_observer(&mut progress);
    if entry.is_some() {
        sources.track_disk_compilation();
    }
    if let Some(sdk_cache) = &mut sdk_cache {
        sources.set_sdk_frontend_cache(sdk_cache);
    }
    if let Some(functions) = &mut functions {
        sources.set_function_frontend_cache(functions);
    }
    let checked = check_fresh_run(path, &options, native, &mut sources, prepare_dependencies);
    if let Some(paths) = watch {
        paths.extend(
            sources
                .input_receipts()
                .into_iter()
                .map(|(path, _, _, _)| path),
        );
    }
    let (context, dependencies) = checked?;
    if !prepare_dependencies && !sources.disk_is_current() {
        return Err("dev preflight inputs changed during check; old run retained".into());
    }
    let evidence = sources.disk_compilation_evidence();
    drop(sources);
    let (pending, generation) = entry
        .and_then(|entry| entry.prepare(&context, evidence?, sdk_cache, functions))
        .map_or((None, None), |(pending, key)| (Some(pending), key));
    Ok(CheckedRun {
        context,
        dependencies,
        pending,
        generation,
    })
}

fn check_fresh_run(
    path: &Path,
    options: &rinzu_compiler::CompilationOptions,
    native: &NativeCliInputs,
    sources: &mut SourceSnapshot<'_>,
    prepare_dependencies: bool,
) -> Result<
    (
        rinzu_compiler::CompilationContext,
        Option<crate::native_package::PreparedDependencies>,
    ),
    String,
> {
    let timing = rinzu_compiler::timing::span("frontend");
    let report = |errors: Vec<rinzu_compiler::Diagnostic>, sources: &SourceSnapshot<'_>| {
        crate::cli_diagnostics::emit_snapshot(&errors, sources);
        "input failed to check".to_owned()
    };
    if !prepare_dependencies
        || !cfg!(all(target_os = "macos", target_arch = "aarch64"))
        || !native.is_empty()
        || !std::thread::available_parallelism().is_ok_and(|count| count.get() > 1)
    {
        let checked = rinzu_compiler::check_input_with_sources(path, options, native, sources)
            .map_err(|errors| report(errors, sources))?;
        return Ok((checked, None));
    }
    std::thread::scope(|scope| {
        let mut worker = None;
        let (ready, pending) = std::sync::mpsc::sync_channel(1);
        let mut pending = Some(pending);
        let checked = rinzu_compiler::check_input_with_sdk_progress(
            path,
            options,
            native,
            sources,
            &mut |manifest, progress| {
                let first = match progress {
                    SdkCheckProgress::Checked(package)
                        if pending.is_some()
                            && crate::native_package::can_prepare_independently(package) =>
                    {
                        Some(package.clone())
                    }
                    SdkCheckProgress::Ready(packages) if !packages.is_empty() => {
                        let _ = ready.send(packages.to_vec());
                        None
                    }
                    _ => return,
                };
                if let Some(pending) = pending.take() {
                    let root = manifest.root.clone();
                    worker = std::thread::Builder::new()
                        .name("rinzu-sdk-build".into())
                        .spawn_scoped(scope, move || {
                            crate::native_package::PreparedDependencies::build(root, first, pending)
                        })
                        .ok();
                }
            },
        );
        drop(ready);
        let checked = checked.map_err(|errors| report(errors, sources));
        drop(timing);
        let build = worker.and_then(|worker| {
            if checked.is_ok() && !worker.is_finished() {
                crate::jit_progress::phase("Waiting for native dependencies");
            }
            let _waiting = rinzu_compiler::timing::span("native.wait_dependencies");
            worker.join().ok().flatten()
        });
        Ok((checked?, build))
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

pub(crate) fn validate_native_inputs(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
) -> Result<(), String> {
    if Target::host().is_some_and(|target| !target.supports_native_packages())
        && (manifest.native != rinzu_compiler::NativeManifest::default()
            || !artifact.native_inputs.is_empty()
            || artifact
                .source_interface
                .as_ref()
                .is_some_and(|source| !source.signatures.is_empty()))
    {
        return Err(format!(
            "native inputs for `{}` are unavailable on {}",
            manifest.name,
            rinzu_runtime::current_target()
        ));
    }
    Ok(())
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
