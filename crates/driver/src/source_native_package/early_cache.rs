use super::{BuiltSourcePackage, facade_input_keys, interface, source_input_keys};
use crate::native_binding::{ReferenceCheck, ReferenceIndex, ReferenceProof};
use crate::native_clang::TargetClang;
use crate::source_package_cache;
use crate::source_receipt::InputSnapshot;
use rinzu_abi::target::Target;
use rinzu_bindgen::native_binding::NativeBindingPlan;
use rinzu_bindgen::source_package::{sha256, validate_source_descriptor};
use rinzu_codegen::source_package::SourcePackageArtifact;
use rinzu_compiler::{NativeCliInputs, PackageKind, ResolvedSdkPackage};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const EVIDENCE: &str = "early-sdk-inputs.json";
const DESCRIPTOR: &str = "source-package-v2.json";
const PROVIDER_SOURCE: &str = "native-addresses.c";

pub(super) struct EarlyCache {
    directory: PathBuf,
    key: String,
    package: String,
    facade: String,
    provider: String,
    sources: usize,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    pub abi: String,
    pub sources: Vec<String>,
    pub provider: Option<String>,
    pub facade: [String; 2],
    pub references: ReferenceProof,
}

#[derive(Clone)]
pub(crate) struct Pending {
    directory: PathBuf,
    key: String,
    package: String,
    files: BTreeMap<String, Vec<u8>>,
}

impl Pending {
    pub(crate) fn store(&self) {
        let mut timing =
            rinzu_compiler::timing::span("native.package.early.store").detail(&self.package);
        timing.set_counter("files", self.files.len() as u64);
        timing.set_counter(
            "bytes",
            self.files.values().map(|file| file.len() as u64).sum(),
        );
        let stored = source_package_cache::store(&self.directory, &self.key, &self.files, None);
        timing.set_counter("stored", u64::from(stored.is_ok()));
    }
}

impl EarlyCache {
    pub(super) fn open(
        package: &ResolvedSdkPackage,
        toolchain: &TargetClang,
        inputs: &NativeCliInputs,
        checked: &BTreeMap<PathBuf, Vec<u8>>,
        dependencies: &BTreeMap<String, String>,
        mode: &str,
    ) -> Option<Self> {
        let manifest = &package.manifest;
        if manifest.kind != PackageKind::Sdk {
            return None;
        }
        let source = package.source.as_ref()?;
        let pin = source.archive_pin.as_ref()?;
        let mut inputs_used = toolchain.fingerprint.clone();
        inputs_used.extend(crate::toolchain_bundle::build_inputs().ok()?);
        inputs_used.extend(dependencies.clone());
        for tool in &manifest.native.config_tools {
            inputs_used.extend(crate::native_config::resolve(tool).ok()?.fingerprint);
        }
        inputs_used.insert(
            "rinzu.compiler-binary".into(),
            interface::compiler_digest().ok()?,
        );
        let checked_key = crate::source_receipt::fingerprint(
            manifest,
            mode,
            &source.module_graph,
            checked,
            inputs,
            &mut inputs_used,
        )
        .ok()?;
        let wire = serde_json::to_vec(&(
            "rinzu.early-sdk-v2",
            pin,
            checked_key,
            std::env::current_dir().ok()?,
            crate::compile_cache::environment()
                .into_iter()
                .collect::<Vec<_>>(),
        ))
        .ok()?;
        let (directory, key) =
            source_package_cache::directory(&manifest.root, &manifest.root, &wire).ok()?;
        let label = &sha256(manifest.name.as_bytes())[..24];
        Some(Self {
            directory,
            key,
            package: manifest.name.clone(),
            facade: toolchain
                .target
                .shared_library_name(&format!("source-{label}")),
            provider: toolchain
                .target
                .shared_library_name(&format!("provider-{label}")),
            sources: manifest.native.sources.len(),
        })
    }

    fn names(&self, plan: &NativeBindingPlan, evidence: &Evidence) -> Option<BTreeSet<String>> {
        if evidence.sources.len() != self.sources {
            return None;
        }
        let mut names: BTreeSet<_> = [
            DESCRIPTOR,
            "source-package.h",
            "source-table.c",
            "source-wrappers.c",
            "native-contract.h",
            "source-input-receipt.json",
            "abi-query.c",
            &self.facade,
            EVIDENCE,
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        names.extend((0..self.sources).map(|index| format!("source-{index}.o")));
        if evidence.provider.is_some() {
            names.insert(PROVIDER_SOURCE.to_owned());
        }
        for image in plan.images.iter().filter(|image| image.bundled) {
            if (image.path == self.provider && evidence.provider.is_none())
                || !names.insert(image.path.clone())
            {
                return None;
            }
        }
        Some(names)
    }

    pub(super) fn restore(
        &self,
        package: &ResolvedSdkPackage,
        application_root: &Path,
        inputs: &NativeCliInputs,
        toolchain: &TargetClang,
        directory: &Path,
        snapshot: &InputSnapshot,
    ) -> Option<BuiltSourcePackage> {
        match self.restore_checked(
            package,
            application_root,
            inputs,
            toolchain,
            directory,
            snapshot,
        ) {
            Ok(built) => {
                let _hit =
                    rinzu_compiler::timing::span("native.package.early.hit").detail(&self.package);
                Some(built)
            }
            Err(reason) => {
                let _ = std::fs::remove_dir_all(directory.join("restored-sdk"));
                let _miss =
                    rinzu_compiler::timing::span("native.package.early.miss").detail(reason);
                None
            }
        }
    }

    fn restore_checked(
        &self,
        package: &ResolvedSdkPackage,
        application_root: &Path,
        inputs: &NativeCliInputs,
        toolchain: &TargetClang,
        directory: &Path,
        snapshot: &InputSnapshot,
    ) -> Result<BuiltSourcePackage, &'static str> {
        let mut lookup =
            rinzu_compiler::timing::span("native.package.early.lookup").detail(&self.package);
        let (files, (descriptor, evidence, plan)) = source_package_cache::completed_with_inventory(
            &self.directory,
            &self.key,
            &[DESCRIPTOR, EVIDENCE],
            |files| {
                let descriptor = validate_source_descriptor(&files[DESCRIPTOR]).ok()?;
                let evidence: Evidence = serde_json::from_slice(&files[EVIDENCE]).ok()?;
                let plan = NativeBindingPlan::decode(&descriptor.interface.package.build_inputs)
                    .ok()??;
                Some((self.names(&plan, &evidence)?, (descriptor, evidence, plan)))
            },
        )
        .ok_or("missing or corrupt cache files")?;
        lookup.set_counter("files", files.len() as u64);
        lookup.set_counter("bytes", files.values().map(|file| file.len() as u64).sum());
        drop(lookup);

        let _verify =
            rinzu_compiler::timing::span("native.package.early.verify").detail(&self.package);
        let wire = &files[DESCRIPTOR];
        let build_inputs = &descriptor.interface.package.build_inputs;
        if descriptor.interface.package.package != self.package
            || descriptor.interface.package.target != toolchain.target.triple()
            || !evidence.references.covers(build_inputs, toolchain)
        {
            return Err("incompatible native artifacts or unproven linker inputs");
        }

        // A rejected generation must not become an include or linker input on fallback.
        let restored = directory.join("restored-sdk");
        std::fs::create_dir_all(directory).map_err(|_| "create build directory")?;
        std::fs::create_dir(&restored).map_err(|_| "create restore directory")?;
        for (name, bytes) in &files {
            if name != EVIDENCE {
                std::fs::write(restored.join(name), bytes).map_err(|_| "restore output files")?;
            }
        }
        crate::source_receipt::verify_seal(&restored, build_inputs).map_err(|_| "artifact seal")?;
        let mut snapshot = snapshot.clone();
        snapshot
            .recorded(&package.manifest.root, &toolchain.sysroot, build_inputs)
            .map_err(|_| "recorded input changed")?;
        if crate::native_contract::package_abi_input_key(&package.manifest, &restored, toolchain)
            .as_ref()
            != Some(&evidence.abi)
        {
            return Err("ABI preprocessing changed");
        }
        if source_input_keys(package, &restored, toolchain, build_inputs).as_ref()
            != Some(&evidence.sources)
        {
            return Err("implementation preprocessing changed");
        }
        if let Some(expected) = &evidence.provider
            && crate::native_binding::provider_input_key(
                toolchain,
                package,
                &restored,
                &self.provider,
            )
            .as_ref()
                != Some(expected)
        {
            return Err("provider preprocessing changed");
        }
        if facade_input_keys(package, &restored, toolchain).as_ref() != Some(&evidence.facade) {
            return Err("facade preprocessing changed");
        }

        let index = crate::cli_diagnostics::capture(|| {
            ReferenceIndex::prepare(
                package,
                application_root,
                inputs,
                &restored,
                toolchain,
                true,
            )
        })
        .0
        .map_err(|_| "native reference selection failed")?;
        if index.proof(&restored).as_ref() != Some(&evidence.references) {
            return Err("native reference selection changed");
        }
        let library = restored.join(&self.facade);
        if !self.verify_images(&restored, toolchain.target, &plan) {
            return Err("native image or signature verification failed");
        }
        snapshot
            .verify()
            .map_err(|_| "input changed during restore")?;
        let fingerprint = build_inputs
            .get(rinzu_bindgen::compilation::DIGEST_KEY)
            .ok_or("missing SDK input fingerprint")?;
        let report = serde_json::json!({
            "schema": "rinzu.source-cache-report-v4", "hit": true, "completed": true,
            "early": true, "inputFingerprint": fingerprint, "descriptorDigest": sha256(wire),
        });
        std::fs::write(
            restored.join("source-cache-report.json"),
            report.to_string(),
        )
        .map_err(|_| "write restore report")?;
        let reference = ReferenceCheck::new(package, application_root, inputs, &restored, &index);
        Ok(BuiltSourcePackage {
            artifact: SourcePackageArtifact {
                facade_digest: Some(sha256(&files[&self.facade])),
                expected_wire: wire.clone(),
                library: Some(library),
            },
            descriptor: restored.join(DESCRIPTOR),
            contract: restored.join("source-package.h"),
            table: restored.join("source-table.c"),
            input_snapshot: Some(snapshot),
            reference_check: Some(reference),
            owner_manifest: Some(package.manifest.clone()),
            pending_early_cache: None,
        })
    }

    pub(super) fn prepare_store(
        self,
        directory: &Path,
        toolchain: &TargetClang,
        build_inputs: &BTreeMap<String, String>,
        evidence: Evidence,
    ) -> Option<Pending> {
        if !evidence.references.covers(build_inputs, toolchain)
            || directory.join(PROVIDER_SOURCE).is_file() != evidence.provider.is_some()
        {
            return None;
        }
        let plan = NativeBindingPlan::decode(build_inputs).ok()??;
        let mut files = BTreeMap::new();
        for name in self.names(&plan, &evidence)? {
            let bytes = if name == EVIDENCE {
                serde_json::to_vec(&evidence).ok()?
            } else {
                std::fs::read(directory.join(&name)).ok()?
            };
            files.insert(name, bytes);
        }
        if !self.verify_images(directory, toolchain.target, &plan) {
            return None;
        }
        Some(Pending {
            directory: self.directory,
            key: self.key,
            package: self.package,
            files,
        })
    }

    fn verify_images(&self, directory: &Path, target: Target, plan: &NativeBindingPlan) -> bool {
        let mut paths = vec![directory.join(&self.facade)];
        for image in plan.images.iter().filter(|image| image.bundled) {
            let path = directory.join(&image.path);
            let Ok(bytes) = std::fs::read(&path) else {
                return false;
            };
            if image.sha256.as_deref() != Some(sha256(&bytes).as_str()) {
                return false;
            }
            if target == Target::X86_64UnknownLinuxGnu
                && !crate::platform::linux::elf::image_info(&path, &bytes).is_ok_and(|current| {
                    current.build_id == image.build_id && current.soname == image.soname
                })
            {
                return false;
            }
            paths.push(path);
        }
        match target {
            Target::Aarch64AppleDarwin => {
                let _timing = rinzu_compiler::timing::span("native.package.early.codesign")
                    .detail(&self.package);
                let paths: Vec<_> = paths.iter().map(PathBuf::as_path).collect();
                crate::platform::macos::verify_code_signatures(&paths)
            }
            Target::X86_64UnknownLinuxGnu => std::fs::read(&paths[0])
                .is_ok_and(|bytes| crate::platform::linux::elf::shared(&bytes).is_ok()),
        }
    }
}
