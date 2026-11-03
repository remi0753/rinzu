use crate::source_native_package::BuiltSourcePackage;
use rinzu_bindgen::source_package::{sha256, validate_source_descriptor};
#[cfg(test)]
use rinzu_codegen::source_package::LoadedSourcePackages;
use rinzu_codegen::source_package::SourcePackageArtifact;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    schema: String,
    version: u32,
    packages: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    package: String,
    descriptor: String,
    library: Option<String>,
    facade_digest: Option<String>,
    files: BTreeMap<String, String>,
}

pub fn save(packages: &[BuiltSourcePackage], destination: &Path) -> Result<PathBuf, String> {
    save_to(packages, &[destination])
}

/// Publish identical verified bytes to all destinations in one preparation.
/// Link/reference validation belongs to the snapshot, not to each copy.
pub fn save_to(packages: &[BuiltSourcePackage], destinations: &[&Path]) -> Result<PathBuf, String> {
    let _timing = rinzu_compiler::timing::span("native.bundle");
    let destination = *destinations
        .first()
        .ok_or("source bundle destination missing")?;
    for destination in destinations {
        std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    }
    let write = |relative: &str, bytes: &[u8]| -> Result<(), String> {
        let _timing = rinzu_compiler::timing::span("native.bundle.write");
        for destination in destinations {
            std::fs::write(destination.join(relative), bytes).map_err(|error| error.to_string())?;
        }
        Ok(())
    };
    let mut entries = Vec::new();
    BuiltSourcePackage::verify_all(packages, None)?;
    for package in packages {
        let descriptor = validate_source_descriptor(&package.artifact.expected_wire)?;
        let directory_name = format!(
            "sdk-{}",
            &sha256(descriptor.interface.package.package.as_bytes())[..24]
        );
        for destination in destinations {
            std::fs::create_dir_all(destination.join(&directory_name))
                .map_err(|error| error.to_string())?;
        }
        let source = package
            .descriptor
            .parent()
            .ok_or("source artifact has no directory")?;
        let mut files = BTreeMap::new();
        crate::source_receipt::verify_seal(source, &descriptor.interface.package.build_inputs)?;
        if descriptor
            .interface
            .package
            .build_inputs
            .contains_key(rinzu_bindgen::compilation::DIGEST_KEY)
        {
            let relative = format!("{directory_name}/source-input-receipt.json");
            let bytes = std::fs::read(source.join("source-input-receipt.json"))
                .map_err(|error| error.to_string())?;
            write(&relative, &bytes)?;
            files.insert(relative, sha256(&bytes));
        }
        for name in [
            "source-package-v2.json",
            "source-package.h",
            "source-table.c",
            "source-wrappers.c",
            "native-contract.h",
        ] {
            let bytes = std::fs::read(source.join(name)).map_err(|error| error.to_string())?;
            if name == "source-package-v2.json" && bytes != package.artifact.expected_wire {
                return Err("generated descriptor changed before bundling".to_owned());
            }
            let relative = format!("{directory_name}/{name}");
            write(&relative, &bytes)?;
            files.insert(relative, sha256(&bytes));
        }
        if let Some(plan) = rinzu_bindgen::native_binding::NativeBindingPlan::decode(
            &descriptor.interface.package.build_inputs,
        )? {
            plan.validate_functions(&descriptor.interface.functions)?;
            for image in plan.images.iter().filter(|image| image.bundled) {
                let bytes =
                    std::fs::read(source.join(&image.path)).map_err(|error| error.to_string())?;
                if image.sha256.as_deref() != Some(sha256(&bytes).as_str()) {
                    return Err("native provider changed before bundling".into());
                }
                let relative = format!("{directory_name}/{}", image.path);
                write(&relative, &bytes)?;
                files.insert(relative, sha256(&bytes));
            }
        }
        let library = package
            .artifact
            .library
            .as_ref()
            .map(|path| -> Result<String, String> {
                let name = path
                    .file_name()
                    .ok_or("facade has no file name")?
                    .to_str()
                    .ok_or("facade name is not UTF-8")?;
                let relative = format!("{directory_name}/{name}");
                let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
                if package.artifact.facade_digest.as_deref() != Some(sha256(&bytes).as_str()) {
                    return Err("source facade changed before bundling".to_owned());
                }
                write(&relative, &bytes)?;
                files.insert(relative.clone(), sha256(&bytes));
                Ok(relative)
            })
            .transpose()?;
        entries.push(Entry {
            package: descriptor.interface.package.package,
            descriptor: format!("{directory_name}/source-package-v2.json"),
            library,
            facade_digest: package.artifact.facade_digest.clone(),
            files,
        });
    }
    entries.sort_by(|a, b| a.package.cmp(&b.package));
    let manifest = destination.join("manifest.json");
    write(
        "manifest.json",
        &serde_json::to_vec_pretty(&Bundle {
            schema: "rinzu.source-package-bundle".to_owned(),
            version: 2,
            packages: entries,
        })
        .map_err(|error| error.to_string())?,
    )?;
    // Saving is preparation, not execution. Verify every byte/path without
    // dlopen; the execution loader validates the physical ABI before publication.
    for destination in destinations {
        verify(&destination.join("manifest.json"))?;
    }
    Ok(manifest)
}

pub fn verify(manifest: &Path) -> Result<(), String> {
    let _timing = rinzu_compiler::timing::span("native.bundle.verify");
    artifacts(manifest)?;
    Ok(())
}

fn confined(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("invalid relative source bundle path".to_owned());
    }
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !path.starts_with(root) {
        return Err("source bundle input escaped its root".to_owned());
    }
    Ok(path)
}

#[cfg(test)]
pub fn load(manifest: &Path) -> Result<LoadedSourcePackages, String> {
    LoadedSourcePackages::load(&artifacts(manifest)?)
        .map_err(|error| format!("load saved source package bundle: {error:#}"))
}

fn artifacts(manifest: &Path) -> Result<Vec<SourcePackageArtifact>, String> {
    let root = manifest
        .parent()
        .ok_or("source bundle manifest has no root")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let bundle: Bundle =
        serde_json::from_slice(&std::fs::read(manifest).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if bundle.schema != "rinzu.source-package-bundle" || bundle.version != 2 {
        return Err("unsupported source package bundle".to_owned());
    }
    let mut identities = BTreeSet::new();
    let mut artifacts = Vec::new();
    for entry in bundle.packages {
        if !identities.insert(entry.package.clone()) {
            return Err("duplicate source bundle package identity".to_owned());
        }
        for (relative, expected) in &entry.files {
            if sha256(
                &std::fs::read(confined(&root, relative)?).map_err(|error| error.to_string())?,
            ) != *expected
            {
                return Err(format!("source bundle input `{relative}` digest mismatch"));
            }
        }
        if !entry.files.contains_key(&entry.descriptor)
            || entry
                .library
                .as_ref()
                .is_some_and(|path| !entry.files.contains_key(path))
        {
            return Err("unverified source bundle artifact path".to_owned());
        }
        let wire = std::fs::read(confined(&root, &entry.descriptor)?)
            .map_err(|error| error.to_string())?;
        let descriptor = validate_source_descriptor(&wire)?;
        if descriptor
            .interface
            .package
            .build_inputs
            .contains_key(rinzu_bindgen::compilation::DIGEST_KEY)
        {
            let directory = Path::new(&entry.descriptor)
                .parent()
                .ok_or("descriptor directory missing")?;
            let relative = directory
                .join("source-input-receipt.json")
                .to_str()
                .ok_or("receipt path must be UTF-8")?
                .to_owned();
            if !entry.files.contains_key(&relative) {
                return Err("unverified source artifact receipt".into());
            }
            confined(&root, &relative)?;
            crate::source_receipt::verify_seal(
                &root.join(directory),
                &descriptor.interface.package.build_inputs,
            )?;
        }
        if descriptor.interface.package.package != entry.package {
            return Err("source bundle descriptor package mismatch".to_owned());
        }
        let has_native = descriptor
            .interface
            .functions
            .iter()
            .any(|function| function.symbol.is_some());
        if has_native != entry.library.is_some() || has_native != entry.facade_digest.is_some() {
            return Err("source bundle code artifact disagrees with its interface".to_owned());
        }
        if let Some(library) = &entry.library
            && entry.files.get(library) != entry.facade_digest.as_ref()
        {
            return Err("source bundle facade digest mismatch".to_owned());
        }
        if let Some(plan) = rinzu_bindgen::native_binding::NativeBindingPlan::decode(
            &descriptor.interface.package.build_inputs,
        )? {
            plan.validate_functions(&descriptor.interface.functions)?;
            let directory = Path::new(&entry.descriptor)
                .parent()
                .ok_or("native descriptor has no directory")?;
            for image in plan.images.iter().filter(|image| image.bundled) {
                let relative = directory
                    .join(&image.path)
                    .to_str()
                    .ok_or("native provider path must be UTF-8")?
                    .to_owned();
                if entry.files.get(&relative) != image.sha256.as_ref() {
                    return Err("unverified bundled native provider".into());
                }
                confined(&root, &relative)?;
            }
        }
        artifacts.push(SourcePackageArtifact {
            library: entry
                .library
                .map(|path| confined(&root, &path))
                .transpose()?,
            expected_wire: wire,
            facade_digest: entry.facade_digest,
        });
    }
    Ok(artifacts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinzu_bindgen::source_package::{FunctionKind, SourceInterface, generate_source_package};

    #[test]
    fn saved_bundle_rejects_modified_files_and_paths_before_publication() {
        let root = std::env::temp_dir().join(format!("rinzu-source-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let mut interface: SourceInterface = serde_json::from_str(include_str!(
            "../../../tests/fixtures/source-package/interface.json"
        ))
        .unwrap();
        let function = &mut interface.functions[0];
        function.symbol = None;
        function.service = Some("reactor.timer".to_owned());
        function.source_type = "async(Int)->Unit".to_owned();
        function.kind = FunctionKind::Async;
        function.c_prototype.clear();
        let generated = generate_source_package(interface, &[]).unwrap();
        for (name, bytes) in [
            ("source-package-v2.json", generated.wire.as_slice()),
            ("source-package.h", generated.header.as_bytes()),
            ("source-table.c", generated.c_table.as_bytes()),
            ("source-wrappers.c", b"/* service */"),
            ("native-contract.h", b"/* contract */"),
        ] {
            std::fs::write(source.join(name), bytes).unwrap();
        }
        let package = BuiltSourcePackage {
            artifact: SourcePackageArtifact {
                library: None,
                expected_wire: generated.wire.clone(),
                facade_digest: None,
            },
            descriptor: source.join("source-package-v2.json"),
            contract: source.join("source-package.h"),
            table: source.join("source-table.c"),
            input_snapshot: None,
            reference_check: None,
            owner_manifest: None,
        };
        let manifest = save(&[package], &root.join("saved")).unwrap();
        assert_eq!(load(&manifest).unwrap().descriptors().len(), 1);
        let original = std::fs::read(&manifest).unwrap();
        for mutation in 0..3 {
            let mut old: serde_json::Value = serde_json::from_slice(&generated.wire).unwrap();
            match mutation {
                0 => old["minor"] = 3.into(),
                1 => {
                    old["interface"]["backend"]
                        .as_object_mut()
                        .unwrap()
                        .remove("native_data_contract");
                }
                2 => {
                    old["interface"]["functions"][0]["conversion_plan"] =
                        serde_json::json!(["identity-pointer"])
                }
                _ => unreachable!(),
            }
            let wire = serde_json::to_vec(&old).unwrap();
            let mut bundle: Bundle = serde_json::from_slice(&original).unwrap();
            let entry = &mut bundle.packages[0];
            std::fs::write(manifest.parent().unwrap().join(&entry.descriptor), &wire).unwrap();
            entry.files.insert(entry.descriptor.clone(), sha256(&wire));
            std::fs::write(&manifest, serde_json::to_vec(&bundle).unwrap()).unwrap();
            assert!(
                verify(&manifest).is_err(),
                "old descriptor mutation {mutation}"
            );
            assert!(load(&manifest).is_err());
        }
        let bundle: Bundle = serde_json::from_slice(&original).unwrap();
        std::fs::write(
            manifest
                .parent()
                .unwrap()
                .join(&bundle.packages[0].descriptor),
            &generated.wire,
        )
        .unwrap();
        std::fs::write(&manifest, &original).unwrap();
        assert!(verify(&manifest).is_ok());
        let bundle: Bundle = serde_json::from_slice(&original).unwrap();
        let header = bundle.packages[0]
            .files
            .keys()
            .find(|name| name.ends_with("source-package.h"))
            .unwrap();
        let header = manifest.parent().unwrap().join(header);
        std::fs::write(&header, b"corrupt").unwrap();
        assert!(load(&manifest).unwrap_err().contains("digest mismatch"));
        std::fs::write(&header, generated.header).unwrap();
        let mut bundle: Bundle = serde_json::from_slice(&original).unwrap();
        bundle.packages[0].descriptor = "../source/source-package-v2.json".to_owned();
        bundle.packages[0].files.insert(
            "../source/source-package-v2.json".to_owned(),
            sha256(&generated.wire),
        );
        std::fs::write(&manifest, serde_json::to_vec(&bundle).unwrap()).unwrap();
        assert!(
            load(&manifest)
                .unwrap_err()
                .contains("relative source bundle path")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
