//! Deploy checked native images without retaining source/cache search paths.
use crate::source_native_package::BuiltSourcePackage;
use object::{Object, ObjectSection};
use rinzu_bindgen::native_binding::NativeBindingPlan;
use rinzu_bindgen::source_package::{generate_source_package, sha256, validate_source_descriptor};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

struct DeploymentTools(BTreeMap<String, (PathBuf, String)>);

impl DeploymentTools {
    fn resolve() -> Result<Self, String> {
        let mut files = BTreeMap::new();
        for name in ["otool", "install_name_tool", "codesign"] {
            let path = if name == "codesign" {
                PathBuf::from("/usr/bin/codesign")
            } else {
                PathBuf::from(
                    output(Command::new("xcrun").args(["--sdk", "macosx", "--find", name]))?.trim(),
                )
            };
            let path = path.canonicalize().map_err(|error| error.to_string())?;
            let digest = sha256(&std::fs::read(&path).map_err(|error| error.to_string())?);
            files.insert(name.into(), (path, digest));
        }
        Ok(Self(files))
    }

    fn command(&self, name: &str) -> Command {
        Command::new(&self.0[name].0)
    }

    fn verify(&self) -> Result<(), String> {
        if Self::resolve()?.0 != self.0 {
            return Err("native deployment tools changed before publication".into());
        }
        Ok(())
    }
}

fn output(command: &mut Command) -> Result<String, String> {
    let result = command.output().map_err(|error| error.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "native deployment failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    String::from_utf8(result.stdout).map_err(|error| error.to_string())
}

fn os_path(path: &Path) -> bool {
    path.starts_with("/usr/lib") || path.starts_with("/System/Library")
}

fn sections(path: &Path) -> Result<BTreeMap<String, (u64, u64, String)>, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let file = object::File::parse(bytes.as_slice()).map_err(|error| error.to_string())?;
    if file.format() != object::BinaryFormat::MachO
        || file.architecture() != object::Architecture::Aarch64
        || file.kind() != object::ObjectKind::Dynamic
    {
        return Err("deployed native image requires an arm64 Mach-O dylib".into());
    }
    file.sections()
        .map(|section| {
            Ok((
                format!(
                    "{:?}:{}",
                    section.kind(),
                    section.name().map_err(|error| error.to_string())?
                ),
                (
                    section.address(),
                    section.size(),
                    sha256(section.data().map_err(|error| error.to_string())?),
                ),
            ))
        })
        .collect()
}

fn commands(path: &Path, tools: &DeploymentTools) -> Result<(Vec<String>, Vec<String>), String> {
    let id = output(tools.command("otool").arg("-D").arg(path))?
        .lines()
        .skip(1)
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned);
    let dependencies = output(tools.command("otool").arg("-L").arg(path))?
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.trim()
                .split_once(" (compatibility version")
                .map(|(path, _)| path.to_owned())
        })
        .filter(|path| Some(path) != id.as_ref())
        .collect();
    let text = output(tools.command("otool").arg("-l").arg(path))?;
    let mut rpaths = Vec::new();
    let mut rpath = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("cmd ") {
            rpath = line == "cmd LC_RPATH";
        }
        if rpath
            && let Some(path) = line
                .strip_prefix("path ")
                .and_then(|path| path.split_once(" (offset ").map(|(path, _)| path))
        {
            rpaths.push(path.into());
            rpath = false;
        }
    }
    Ok((dependencies, rpaths))
}

fn expand(path: &str, directory: &Path) -> Option<PathBuf> {
    if let Some(path) = path.strip_prefix("@loader_path/") {
        Some(directory.join(path))
    } else if Path::new(path).is_absolute() {
        Some(path.into())
    } else {
        None
    }
}

fn dependency(
    image: &Path,
    name: &str,
    rpaths: &[String],
    searches: &[PathBuf],
) -> Result<PathBuf, String> {
    let parent = image.parent().ok_or("native image has no directory")?;
    let candidates = if let Some(name) = name.strip_prefix("@rpath/") {
        rpaths
            .iter()
            .filter_map(|path| expand(path, parent))
            .chain(searches.iter().cloned())
            .map(|path| path.join(name))
            .collect::<Vec<_>>()
    } else {
        expand(name, parent).into_iter().collect()
    };
    candidates.into_iter().find(|path| path.is_file()).ok_or_else(|| format!("cannot deploy native dependency `{name}` of {}; use a resolvable absolute/@loader_path/@rpath install name", image.display()))?.canonicalize().map_err(|error| error.to_string())
}

struct Image {
    original: String,
    name: String,
    bytes: String,
    uuid: String,
    sections: BTreeMap<String, (u64, u64, String)>,
    changes: Vec<(String, PathBuf)>,
    rpaths: Vec<String>,
}

fn collect(
    path: &Path,
    generated: &Path,
    searches: &[PathBuf],
    images: &mut BTreeMap<PathBuf, Image>,
    os: &mut BTreeSet<String>,
    tools: &DeploymentTools,
) -> Result<(), String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if images.contains_key(&path) {
        return Ok(());
    }
    let info = crate::native_binding::image_info(&path)?;
    let identity = path.strip_prefix(generated).map_or_else(
        |_| path.to_string_lossy().into_owned(),
        |path| format!("generated:{}", path.display()),
    );
    let (dependencies, rpaths) = commands(&path, tools)?;
    let mut changes = Vec::new();
    for name in dependencies {
        if os_path(Path::new(&name)) {
            os.insert(name);
            continue;
        }
        changes.push((name.clone(), dependency(&path, &name, &rpaths, searches)?));
    }
    images.insert(
        path.clone(),
        Image {
            original: identity.clone(),
            name: format!("native-{}.dylib", &sha256(identity.as_bytes())[..16]),
            bytes: info.sha256.ok_or("deployed image digest missing")?,
            uuid: info.uuid,
            sections: sections(&path)?,
            changes: changes.clone(),
            rpaths,
        },
    );
    for (_, dependency) in changes {
        collect(&dependency, generated, searches, images, os, tools)?;
    }
    Ok(())
}

pub fn prepare(
    packages: &[BuiltSourcePackage],
    destination: &Path,
) -> Result<Vec<BuiltSourcePackage>, String> {
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    packages
        .iter()
        .enumerate()
        .map(|(index, package)| deploy(package, &destination.join(index.to_string())))
        .collect()
}

fn deploy(package: &BuiltSourcePackage, destination: &Path) -> Result<BuiltSourcePackage, String> {
    package.verify_inputs()?;
    let descriptor = validate_source_descriptor(&package.artifact.expected_wire)?;
    let Some(mut plan) = NativeBindingPlan::decode(&descriptor.interface.package.build_inputs)?
    else {
        return Ok(package.clone());
    };
    let source = package
        .descriptor
        .parent()
        .ok_or("native descriptor directory missing")?;
    let manifest = package
        .owner_manifest
        .as_ref()
        .ok_or("native owner manifest missing")?;
    std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    let mut searches: Vec<_> = manifest
        .native
        .search_paths
        .iter()
        .map(|path| manifest.root.join(path))
        .collect();
    if let Some(paths) = descriptor
        .interface
        .package
        .build_inputs
        .get("cli.search-paths")
    {
        searches.extend(
            serde_json::from_str::<Vec<PathBuf>>(paths)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|path| manifest.root.join(path)),
        );
    }
    let mut images = BTreeMap::new();
    let mut os = BTreeSet::new();
    let tools = DeploymentTools::resolve()?;
    for image in &plan.images {
        if !image.bundled && os_path(Path::new(&image.path)) {
            os.insert(image.path.clone());
            continue;
        }
        let path = if image.bundled {
            source.join(&image.path)
        } else {
            PathBuf::from(&image.path)
        };
        collect(
            &path,
            &source.canonicalize().map_err(|error| error.to_string())?,
            &searches,
            &mut images,
            &mut os,
            &tools,
        )?;
    }
    let mut transformed = BTreeMap::new();
    let mut recipe = BTreeMap::new();
    for (path, image) in &images {
        let target = destination.join(&image.name);
        let current = std::fs::read(path).map_err(|error| error.to_string())?;
        if sha256(&current) != image.bytes {
            return Err("native image changed during deployment".into());
        }
        std::fs::write(&target, current).map_err(|error| error.to_string())?;
        let mut command = tools.command("install_name_tool");
        command
            .arg("-id")
            .arg(format!("@loader_path/{}", image.name));
        for (old, dependency) in &image.changes {
            command
                .arg("-change")
                .arg(old)
                .arg(format!("@loader_path/{}", images[dependency].name));
        }
        for rpath in &image.rpaths {
            command.arg("-delete_rpath").arg(rpath);
        }
        command.arg(&target);
        output(&mut command)?;
        output(
            tools
                .command("codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(&target),
        )?;
        let mut info = crate::native_binding::image_info(&target)?;
        if info.uuid != image.uuid || sections(&target)? != image.sections {
            return Err("native deployment changed code/data or UUID".into());
        }
        let (dependencies, rpaths) = commands(&target, &tools)?;
        if !rpaths.is_empty()
            || dependencies
                .iter()
                .any(|path| !os_path(Path::new(path)) && !path.starts_with("@loader_path/"))
        {
            return Err("deployed native image still has an external search path".into());
        }
        info.path = image.name.clone();
        info.bundled = true;
        recipe.insert(image.name.clone(), serde_json::json!({"original": image.original, "originalSha256": image.bytes, "deployedSha256": info.sha256, "uuid": image.uuid, "sections": image.sections, "dependencies": dependencies, "signed": "ad-hoc"}));
        transformed.insert(path.clone(), info);
    }
    let original = plan.clone();
    let mut originals = Vec::new();
    for image in &mut plan.images {
        let path = if image.bundled {
            source.join(&image.path)
        } else {
            PathBuf::from(&image.path)
        };
        if os_path(&path) {
            originals.push(None);
            continue;
        }
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        *image = transformed[&path].clone();
        originals.push(Some(path));
    }
    for slot in &mut plan.slots {
        let path = if original.owner_bundled(slot) {
            source.join(&slot.owner_path)
        } else {
            PathBuf::from(&slot.owner_path)
        };
        if os_path(&path) {
            continue;
        }
        let path = path.canonicalize().map_err(|error| error.to_string())?;
        let owner = transformed
            .get(&path)
            .ok_or("native terminal owner is missing from deployment closure")?;
        slot.owner_path = owner.path.clone();
        slot.owner_sha256 = owner.sha256.clone();
        slot.owner_uuid = owner.uuid.clone();
    }
    for (path, image) in &transformed {
        if !originals
            .iter()
            .any(|original| original.as_ref() == Some(path))
        {
            plan.images.push(image.clone());
        }
    }
    plan.version = 2;
    let mut interface = descriptor.interface.clone();
    let previous =
        interface.package.build_inputs[rinzu_bindgen::native_binding::DIGEST_KEY].clone();
    let digest = plan.encode(&mut interface.package.build_inputs)?;
    let mut wrappers = std::fs::read_to_string(source.join("source-wrappers.c"))
        .map_err(|error| error.to_string())?;
    if wrappers.matches(&previous).count() != 1 {
        return Err("native initializer digest cannot be rewritten unambiguously".into());
    }
    wrappers = wrappers.replace(&previous, &digest);
    let toolchain = crate::native_clang::TargetClang::resolve()?;
    interface.package.build_inputs.insert("native.deployment-v1".into(), serde_json::to_string(&serde_json::json!({"schema": "rinzu.native-deployment-v1", "images": recipe, "osDependencies": os, "compile": "-dynamiclib -O2 -Wall -Wextra -Werror -Wcast-function-type-strict -Wincompatible-function-pointer-types-strict -fno-builtin -fno-lto", "toolchain": toolchain.fingerprint, "tools": tools.0})).map_err(|error| error.to_string())?);
    let mut input: serde_json::Value =
        serde_json::from_str(&interface.package.build_inputs[rinzu_bindgen::compilation::KEY])
            .map_err(|error| error.to_string())?;
    let mut closure = interface.package.build_inputs.clone();
    closure.remove(rinzu_bindgen::compilation::KEY);
    closure.remove(rinzu_bindgen::compilation::DIGEST_KEY);
    input["inputs"] = serde_json::to_value(closure).map_err(|error| error.to_string())?;
    input["purpose"] = "aot-deployment".into();
    rinzu_bindgen::compilation::encode(&mut interface.package.build_inputs, input)?;
    let mut generated = generate_source_package(
        interface,
        &serde_json::to_vec(&(descriptor.native_artifact_digest, &plan))
            .map_err(|error| error.to_string())?,
    )?;
    generated
        .header
        .insert_str(0, &crate::source_native_package::header_includes(manifest)?);
    let mut files = BTreeMap::from([
        ("source-package-v2.json".into(), generated.wire.clone()),
        (
            "source-package.h".into(),
            generated.header.as_bytes().to_vec(),
        ),
        (
            "source-table.c".into(),
            generated.c_table.as_bytes().to_vec(),
        ),
        ("source-wrappers.c".into(), wrappers.into_bytes()),
        (
            "native-contract.h".into(),
            std::fs::read(source.join("native-contract.h")).map_err(|error| error.to_string())?,
        ),
    ]);
    for (name, bytes) in &files {
        std::fs::write(destination.join(name), bytes).map_err(|error| error.to_string())?;
    }
    let library_name = package
        .artifact
        .library
        .as_ref()
        .and_then(|path| path.file_name())
        .ok_or("deployed facade filename missing")?;
    let library = destination.join(library_name);
    let mut command = toolchain.command();
    command
        .args([
            "-dynamiclib",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wcast-function-type-strict",
            "-Wincompatible-function-pointer-types-strict",
            "-fno-builtin",
            "-fno-lto",
            "-install_name",
        ])
        .arg(format!("@loader_path/{}", library_name.to_string_lossy()));
    command
        .arg("-I")
        .arg(&manifest.root)
        .arg("-I")
        .arg(destination)
        .arg(destination.join("source-wrappers.c"))
        .arg(destination.join("source-table.c"))
        .arg("-o")
        .arg(&library);
    crate::source_native_package::add_package_flags(
        &mut command,
        &rinzu_compiler::ResolvedSdkPackage {
            manifest: manifest.clone(),
            native_imports: Vec::new(),
            source: None,
        },
        false,
    )?;
    output(&mut command)?;
    output(
        tools
            .command("codesign")
            .args(["--force", "--sign", "-", "--timestamp=none"])
            .arg(&library),
    )?;
    toolchain.verify()?;
    tools.verify()?;
    for (path, image) in &images {
        if sha256(&std::fs::read(path).map_err(|error| error.to_string())?) != image.bytes {
            return Err("native dependency changed before deployment publication".into());
        }
    }
    package.verify_inputs()?;
    files.insert(
        library_name.to_string_lossy().into_owned(),
        std::fs::read(&library).map_err(|error| error.to_string())?,
    );
    for image in plan.images.iter().filter(|image| image.bundled) {
        files.insert(
            image.path.clone(),
            std::fs::read(destination.join(&image.path)).map_err(|error| error.to_string())?,
        );
    }
    crate::source_receipt::seal(
        destination,
        &generated.descriptor.interface.package.build_inputs,
        &files,
    )?;
    let mut result = package.clone();
    result.artifact.expected_wire = generated.wire;
    result.artifact.facade_digest =
        Some(sha256(&files[&library_name.to_string_lossy().into_owned()]));
    result.artifact.library = Some(library);
    result.descriptor = destination.join("source-package-v2.json");
    result.contract = destination.join("source-package.h");
    result.table = destination.join("source-table.c");
    Ok(result)
}
