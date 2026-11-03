use std::path::{Path, PathBuf};

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
    let mut built = build_unlocked(
        &artifact.sdk_packages,
        &manifest.root,
        &artifact.native_inputs,
    )?;
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
    let directory = std::env::temp_dir().join(format!(
        "rinzu-native-packages-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("read system clock: {error}"))?
            .as_nanos()
    ));
    std::fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "create native package output {}: {error}",
            directory.display()
        )
    })?;
    let mut scratch = Scratch {
        path: directory.clone(),
        keep: false,
    };
    let artifacts = Vec::new();
    let default_jobs = std::thread::available_parallelism().map_or(4, |count| count.get().min(4));
    let jobs = match std::env::var("RINZU_NATIVE_JOBS") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|jobs| (1..=4).contains(jobs))
            .ok_or("RINZU_NATIVE_JOBS must be an integer from 1 to 4")?,
        Err(std::env::VarError::NotPresent) => default_jobs,
        Err(_) => return Err("RINZU_NATIVE_JOBS must be an integer from 1 to 4".into()),
    };
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
    let (source_packages, dependency_digests) =
        crate::native_build_scheduler::build(&names, &dependencies, jobs, |index, digests| {
            let built = crate::source_native_package::build_with_dependencies(
                &packages[index],
                application_root,
                inputs,
                &outputs[index],
                digests,
            )?;
            let digest = rinzu_bindgen::source_package::sha256(&built.artifact.expected_wire);
            Ok((built, digest))
        })?;
    drop(packages_timing);
    if !inputs.is_empty() {
        let index =
            crate::native_binding::ReferenceIndex::for_cli(application_root, inputs, &directory)?;
        explicit_link_inputs = index.inputs;
    }
    scratch.keep = true;
    Ok(BuiltNativePackages {
        directory,
        artifacts,
        source_packages,
        dependency_digests,
        explicit_link_inputs,
        context_inputs: Default::default(),
        input_snapshot: None,
    })
}
