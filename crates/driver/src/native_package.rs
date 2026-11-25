use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc::Receiver};
use std::thread::ScopedJoinHandle;

use rinzu_codegen::NativePackageArtifact;
use rinzu_compiler::ResolvedSdkPackage;

pub struct BuiltNativePackages {
    pub directory: PathBuf,
    pub artifacts: Vec<NativePackageArtifact>,
    pub source_packages: Vec<crate::source_native_package::BuiltSourcePackage>,
    pub dependency_digests: std::collections::BTreeMap<String, String>,
    pub explicit_link_inputs: std::collections::BTreeMap<String, String>,
    pub context_inputs: std::collections::BTreeMap<String, String>,
    pub input_snapshot: Option<crate::source_receipt::InputSnapshot>,
}

struct Scratch {
    path: PathBuf,
    keep: bool,
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn create_native_directory() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    loop {
        let directory = std::env::temp_dir().join(format!(
            "rinzu-native-packages-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        // Exclusive creation also handles stale directories after PID reuse.
        // A clock reading and create_dir_all cannot establish ownership.
        match builder.create(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "create native package output {}: {error}",
                    directory.display()
                ));
            }
        }
    }
}

impl BuiltNativePackages {
    pub fn load_source_packages(
        &self,
    ) -> Result<rinzu_codegen::source_package::LoadedSourcePackages, String> {
        self.verify_inputs()?;
        let artifacts = self
            .source_packages
            .iter()
            .map(|package| package.artifact.clone())
            .collect::<Vec<_>>();
        rinzu_codegen::source_package::LoadedSourcePackages::load(&artifacts)
            .map_err(|error| format!("load generated source SDK packages: {error:#}"))
    }

    pub fn validate_source_packages(&self) -> Result<(), String> {
        self.load_source_packages()?;
        Ok(())
    }

    pub fn verify_inputs(&self) -> Result<(), String> {
        crate::source_native_package::BuiltSourcePackage::verify_all(
            &self.source_packages,
            self.input_snapshot.as_ref(),
        )
    }

    pub fn context(
        &mut self,
        manifest: &rinzu_compiler::Manifest,
        artifact: &rinzu_compiler::FrontendArtifact,
        own: Option<&crate::source_native_package::BuiltSourcePackage>,
    ) -> Result<(), String> {
        let mut snapshot = crate::source_receipt::InputSnapshot::checked(
            &artifact.checked_inputs,
            &artifact.module_graph,
        )?;
        if let Some(previous) = &self.input_snapshot {
            snapshot.merge(previous)?;
        }
        for package in self.source_packages.iter().chain(own) {
            if let Some(previous) = &package.input_snapshot {
                snapshot.merge(previous)?;
            }
        }
        snapshot.native(manifest, &artifact.native_inputs)?;
        let mut inputs = if !self.source_packages.is_empty()
            || own.is_some()
            || !artifact.native_inputs.is_empty()
        {
            crate::native_clang::TargetClang::resolve()?.fingerprint
        } else {
            Default::default()
        };
        inputs.extend(self.explicit_link_inputs.clone());
        inputs.extend(crate::toolchain_bundle::build_inputs()?);
        for package in &self.source_packages {
            let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
                &package.artifact.expected_wire,
            )?;
            inputs.insert(
                format!("dependency:{}", descriptor.interface.package.package),
                rinzu_bindgen::source_package::sha256(&package.artifact.expected_wire),
            );
        }
        if let Some(own) = own {
            let descriptor = rinzu_bindgen::source_package::validate_source_descriptor(
                &own.artifact.expected_wire,
            )?;
            inputs.extend(descriptor.interface.package.build_inputs);
        }
        inputs.insert(
            "rinzu.compiler-binary".into(),
            snapshot
                .captured_digest(&std::env::current_exe().map_err(|error| error.to_string())?)?,
        );
        crate::source_receipt::fingerprint(
            manifest,
            if artifact.is_file_input {
                "File"
            } else {
                "Package"
            },
            &artifact.module_graph,
            &artifact.checked_inputs,
            &artifact.native_inputs,
            &mut inputs,
        )?;
        std::fs::write(
            self.directory.join("compilation-input.json"),
            inputs[rinzu_bindgen::compilation::KEY].as_bytes(),
        )
        .map_err(|error| error.to_string())?;
        self.context_inputs = inputs;
        let sysroot = self
            .context_inputs
            .get("clang.sysroot")
            .map_or(Path::new(""), |path| Path::new(path));
        snapshot.recorded(&manifest.root, sysroot, &self.context_inputs)?;
        self.input_snapshot = Some(snapshot);
        Ok(())
    }
}

/// Build dependency interfaces and the application's own native interface through
/// the same generator/loader contract, without treating the application as an SDK.
pub fn build_artifact(
    manifest: &rinzu_compiler::Manifest,
    artifact: &rinzu_compiler::FrontendArtifact,
) -> Result<BuiltNativePackages, String> {
    build_artifact_with_dependencies(manifest, artifact, None)
}

/// A speculative SDK build owns scratch files until the checked application
/// consumes it. Frontend failures never publish it or emit its diagnostics.
pub(crate) struct PreparedDependencies {
    root: PathBuf,
    packages: Vec<ResolvedSdkPackage>,
    result: Option<Result<BuiltNativePackages, String>>,
    diagnostics: Vec<String>,
}

pub(crate) fn can_prepare_independently(package: &ResolvedSdkPackage) -> bool {
    package.source.is_some()
        && package.native_imports.is_empty()
        && package.manifest.dependencies.is_empty()
        && rinzu_compiler::standard_library::dependencies(&package.manifest.name).is_empty()
        && !package.manifest.native.sources.is_empty()
        && package.manifest.native.config_tools.is_empty()
}

fn same_checked_sdk(before: &ResolvedSdkPackage, after: &ResolvedSdkPackage) -> bool {
    before.manifest.name == after.manifest.name
        && before.manifest.root == after.manifest.root
        && before
            .source
            .as_ref()
            .zip(after.source.as_ref())
            .is_some_and(|(before, after)| Arc::ptr_eq(before, after))
}

type SdkBuild = Result<crate::source_native_package::BuiltSourcePackage, String>;
type SdkWorker<'scope> = ScopedJoinHandle<'scope, (SdkBuild, Vec<String>)>;

struct EarlySdk<'scope> {
    package: ResolvedSdkPackage,
    worker: Mutex<Option<SdkWorker<'scope>>>,
}

impl PreparedDependencies {
    pub(crate) fn build(
        root: PathBuf,
        first: Option<ResolvedSdkPackage>,
        ready: Receiver<Vec<ResolvedSdkPackage>>,
    ) -> Option<Self> {
        let _timing = rinzu_compiler::timing::span("native.build.early_dependencies");
        let mut scratch = create_native_directory().map(|path| Scratch { path, keep: false });
        let jobs = native_jobs();
        let inputs = rinzu_compiler::NativeCliInputs::default();
        let (packages, result, diagnostics) = std::thread::scope(|scope| {
            let early = if let (Ok(scratch), Ok(jobs), Some(package)) =
                (&scratch, &jobs, first.filter(can_prepare_independently))
                && *jobs > 1
            {
                let output = scratch.path.join("early-source");
                let selected = package.clone();
                let root = &root;
                let inputs = &inputs;
                std::thread::Builder::new()
                    .name("rinzu-sdk-early-source".into())
                    .stack_size(8 * 1024 * 1024)
                    .spawn_scoped(scope, move || {
                        crate::cli_diagnostics::capture(|| {
                            crate::source_native_package::build_with_dependencies(
                                &selected,
                                root,
                                inputs,
                                &output,
                                &Default::default(),
                            )
                        })
                    })
                    .ok()
                    .map(|worker| EarlySdk {
                        package,
                        worker: Mutex::new(Some(worker)),
                    })
            } else {
                None
            };
            let packages = ready.recv().ok()?;
            let (result, diagnostics) = crate::cli_diagnostics::capture(|| {
                let scratch = scratch.as_ref().map_err(Clone::clone)?;
                let jobs = *jobs.as_ref().map_err(Clone::clone)?;
                build_into_directory(
                    &packages,
                    &root,
                    &inputs,
                    &scratch.path,
                    jobs,
                    early.as_ref(),
                )
            });
            Some((packages, result, diagnostics))
        })?;
        if result.is_ok()
            && let Ok(scratch) = &mut scratch
        {
            scratch.keep = true;
        }
        Some(Self {
            root,
            packages,
            result: Some(result),
            diagnostics,
        })
    }

    fn take_for(
        &mut self,
        manifest: &rinzu_compiler::Manifest,
        artifact: &rinzu_compiler::FrontendArtifact,
    ) -> Option<Result<BuiltNativePackages, String>> {
        if self.root != manifest.root
            || !artifact.native_inputs.is_empty()
            || self.packages.len() != artifact.sdk_packages.len()
            || !self
                .packages
                .iter()
                .zip(&artifact.sdk_packages)
                .all(|(before, after)| same_checked_sdk(before, after))
        {
            return None;
        }
        for block in &self.diagnostics {
            crate::cli_diagnostics::write_block(block);
        }
        self.result.take()
    }
}

impl Drop for PreparedDependencies {
    fn drop(&mut self) {
        if let Some(Ok(built)) = &self.result {
            let _ = std::fs::remove_dir_all(&built.directory);
        }
    }
}

pub(crate) fn build_artifact_with_dependencies(
    manifest: &rinzu_compiler::Manifest,
    artifact: &rinzu_compiler::FrontendArtifact,
    mut dependencies: Option<PreparedDependencies>,
) -> Result<BuiltNativePackages, String> {
    crate::input::validate_native_inputs(manifest, artifact)?;
    let _timing = rinzu_compiler::timing::span("native.build");
    let mut snapshot = crate::source_receipt::InputSnapshot::checked(
        &artifact.checked_inputs,
        &artifact.module_graph,
    )?;
    snapshot.native(manifest, &artifact.native_inputs)?;
    for package in &artifact.sdk_packages {
        snapshot.native(
            &package.manifest,
            &rinzu_compiler::NativeCliInputs::default(),
        )?;
    }
    // Each owner validates its ABI with the same toolchain used to compile it.
    // A separate artifact-wide pass would repeat those Clang queries.
    let mut built = match dependencies
        .as_mut()
        .and_then(|built| built.take_for(manifest, artifact))
    {
        Some(built) => built?,
        None => build_unlocked(
            &artifact.sdk_packages,
            &manifest.root,
            &artifact.native_inputs,
        )?,
    };
    let mut scratch = Scratch {
        path: built.directory.clone(),
        keep: false,
    };
    if let Some(source) = &artifact.source_interface {
        let package = ResolvedSdkPackage {
            manifest: manifest.clone(),
            native_imports: Vec::new(),
            source: Some(source.clone()),
        };
        let directory = built.directory.join("root-native");
        let result = crate::source_native_package::build_with_mode(
            &package,
            &manifest.root,
            &artifact.native_inputs,
            &directory,
            &built.dependency_digests,
            if artifact.is_file_input {
                "File"
            } else {
                "Package"
            },
        );
        match result {
            Ok(owner) => {
                built.context(manifest, artifact, Some(&owner))?;
                if !artifact.is_file_input
                    && !crate::package_tests::is_file_worker()
                    && let Err(error) = crate::package_commands::check_sdk_lock(
                        &package,
                        &artifact.native_inputs,
                        &artifact.sdk_packages,
                        &built,
                        &owner,
                    )
                {
                    let _ = std::fs::remove_dir_all(&built.directory);
                    return Err(error);
                }
                built.source_packages.push(owner);
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&built.directory);
                return Err(error);
            }
        }
    } else {
        built.context(manifest, artifact, None)?;
        if !artifact.is_file_input
            && !crate::package_tests::is_file_worker()
            && let Err(error) = crate::package_commands::check_application_lock(
                &manifest.root,
                &artifact.native_inputs,
                &artifact.sdk_packages,
                &built,
            )
        {
            let _ = std::fs::remove_dir_all(&built.directory);
            return Err(error);
        }
    }
    if !artifact.native_inputs.is_empty()
        && !artifact.source_interface.as_ref().is_some_and(|source| {
            source.signatures.iter().any(|signature| {
                matches!(
                    signature.binding,
                    rinzu_compiler::ast::NativeBinding::Symbol(_)
                )
            })
        })
    {
        let result = crate::native_binding::ReferenceIndex::for_cli(
            &manifest.root,
            &artifact.native_inputs,
            &built.directory,
        )
        .and_then(|index| index.validate_link(&built.directory));
        if let Err(error) = result {
            let _ = std::fs::remove_dir_all(&built.directory);
            return Err(error);
        }
    }
    let sysroot = built
        .context_inputs
        .get("clang.sysroot")
        .map_or(Path::new(""), |path| Path::new(path));
    if let Some(context) = &built.input_snapshot {
        snapshot.merge(context)?;
    }
    snapshot.recorded(&manifest.root, sysroot, &built.context_inputs)?;
    snapshot.verify()?;
    built.input_snapshot = Some(snapshot);
    scratch.keep = true;
    Ok(built)
}

pub fn build_with_native_inputs(
    packages: &[ResolvedSdkPackage],
    application_root: &Path,
    inputs: &rinzu_compiler::NativeCliInputs,
) -> Result<BuiltNativePackages, String> {
    let built = build_unlocked(packages, application_root, inputs)?;
    if let Err(error) =
        crate::package_commands::check_application_lock(application_root, inputs, packages, &built)
    {
        let _ = std::fs::remove_dir_all(&built.directory);
        return Err(error);
    }
    Ok(built)
}

/// Explicit lock refresh must inspect the fresh graph even when an old lock is stale.
pub fn build_unlocked(
    packages: &[ResolvedSdkPackage],
    application_root: &Path,
    inputs: &rinzu_compiler::NativeCliInputs,
) -> Result<BuiltNativePackages, String> {
    let path = create_native_directory()?;
    let mut scratch = Scratch { path, keep: false };
    let result = build_into_directory(
        packages,
        application_root,
        inputs,
        &scratch.path,
        native_jobs()?,
        None,
    )?;
    scratch.keep = true;
    Ok(result)
}

fn native_jobs() -> Result<usize, String> {
    let default_jobs = std::thread::available_parallelism().map_or(4, |count| count.get().min(4));
    match std::env::var("RINZU_NATIVE_JOBS") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|jobs| (1..=4).contains(jobs))
            .ok_or_else(|| "RINZU_NATIVE_JOBS must be an integer from 1 to 4".into()),
        Err(std::env::VarError::NotPresent) => Ok(default_jobs),
        Err(_) => Err("RINZU_NATIVE_JOBS must be an integer from 1 to 4".into()),
    }
}

fn build_into_directory(
    packages: &[ResolvedSdkPackage],
    application_root: &Path,
    inputs: &rinzu_compiler::NativeCliInputs,
    directory: &Path,
    jobs: usize,
    early: Option<&EarlySdk<'_>>,
) -> Result<BuiltNativePackages, String> {
    let artifacts = Vec::new();
    let mut explicit_link_inputs = std::collections::BTreeMap::new();
    let mut dependencies = Vec::new();
    let mut outputs = Vec::new();
    for (index, package) in packages.iter().enumerate() {
        if package.source.is_none() || !package.native_imports.is_empty() {
            return Err(format!(
                "SDK package {} requires checked source declarations",
                package.manifest.name
            ));
        }
        let mut required: std::collections::BTreeSet<_> = package
            .manifest
            .dependencies
            .iter()
            .map(|dependency| dependency.name.clone())
            .collect();
        let standard = rinzu_compiler::standard_library::dependencies(&package.manifest.name);
        if !standard.is_empty()
            && rinzu_compiler::standard_library::is_standard_manifest(&package.manifest)?
        {
            required.extend(standard.iter().map(|name| (*name).to_owned()));
        }
        dependencies.push(required);
        let output = directory.join(format!("source-{index}"));
        std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
        outputs.push(output);
    }
    let packages_timing = rinzu_compiler::timing::span("native.build.packages");
    let names: Vec<_> = packages
        .iter()
        .map(|package| package.manifest.name.clone())
        .collect();
    let priority = early.and_then(|early| {
        packages
            .iter()
            .position(|package| same_checked_sdk(&early.package, package))
    });
    if priority.is_none()
        && let Some(early) = early
        && let Some(worker) = early.worker.lock().unwrap().take()
    {
        let _ = worker.join();
    }
    let (source_packages, dependency_digests) = crate::native_build_scheduler::build(
        &names,
        &dependencies,
        jobs,
        priority,
        |index, digests| {
            let package = &packages[index];
            let built = if let Some(early) = early
                && Some(index) == priority
                && let Some(worker) = early.worker.lock().unwrap().take()
            {
                let (result, diagnostics) = worker
                    .join()
                    .map_err(|_| "early native SDK compiler worker panicked".to_owned())?;
                for block in diagnostics {
                    crate::cli_diagnostics::write_block(&block);
                }
                result?
            } else {
                crate::source_native_package::build_with_dependencies(
                    package,
                    application_root,
                    inputs,
                    &outputs[index],
                    digests,
                )?
            };
            let digest = rinzu_bindgen::source_package::sha256(&built.artifact.expected_wire);
            Ok((built, digest))
        },
    )?;
    drop(packages_timing);
    if !inputs.is_empty() {
        let index =
            crate::native_binding::ReferenceIndex::for_cli(application_root, inputs, directory)?;
        explicit_link_inputs = index.inputs;
    }
    Ok(BuiltNativePackages {
        directory: directory.to_owned(),
        artifacts,
        source_packages,
        dependency_digests,
        explicit_link_inputs,
        context_inputs: Default::default(),
        input_snapshot: None,
    })
}

#[cfg(test)]
mod scratch_tests {
    use super::create_native_directory;

    #[test]
    fn parallel_builds_own_independent_scratch_outputs_and_cleanup() {
        let barrier = std::sync::Barrier::new(16);
        let directories = std::thread::scope(|scope| {
            (0..16)
                .map(|index| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        let directory = create_native_directory().unwrap();
                        std::fs::write(directory.join("source-0.o"), index.to_string()).unwrap();
                        (index, directory)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            directories
                .iter()
                .map(|(_, path)| path)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            directories.len()
        );
        for (index, directory) in directories {
            assert_eq!(
                std::fs::read_to_string(directory.join("source-0.o")).unwrap(),
                index.to_string()
            );
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
