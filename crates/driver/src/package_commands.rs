//! Fresh local source graphs, dependency locks, and public API reports.
mod documentation;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rinzu_bindgen::source_package::{
    FunctionRecord, SourceDescriptor, SourceTemplateRecord, SourceTypeRecord, TypeRecord, sha256,
    validate_source_descriptor,
};
use rinzu_compiler::{
    FrontendArtifact, Manifest, NativeCliInputs, PackageKind, ResolvedSdkPackage,
};
use serde::{Deserialize, Serialize};

const LOCK_SCHEMA: &str = "rinzu.package-lock";
const API_SCHEMA: &str = "rinzu.package-api";
const VERSION: u32 = 2;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPlan {
    name: String,
    language: String,
    kind: String,
    entry: PathBuf,
    exports: Vec<String>,
    dependencies: BTreeMap<String, PathBuf>,
    native: BTreeMap<String, Vec<String>>,
}

fn plan(manifest: &Manifest) -> ManifestPlan {
    let native = &manifest.native;
    let paths = |values: &[PathBuf]| values.iter().map(|p| p.to_string_lossy().into()).collect();
    ManifestPlan {
        name: manifest.name.clone(),
        language: manifest.language.clone(),
        kind: match manifest.kind {
            PackageKind::Application => "application",
            PackageKind::Sdk => "sdk",
        }
        .into(),
        entry: manifest.entry.clone(),
        exports: manifest.exports.modules.clone(),
        dependencies: manifest
            .dependencies
            .iter()
            .map(|d| (d.name.clone(), d.path.clone()))
            .collect(),
        native: BTreeMap::from([
            ("sources".into(), paths(&native.sources)),
            ("headers".into(), paths(&native.headers)),
            ("objects".into(), paths(&native.objects)),
            ("includeDirs".into(), paths(&native.include_dirs)),
            ("searchPaths".into(), paths(&native.search_paths)),
            ("libraries".into(), native.libraries.clone()),
            ("frameworks".into(), native.frameworks.clone()),
            ("configTools".into(), native.config_tools.clone()),
        ]),
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedPackage {
    path: PathBuf,
    manifest: ManifestPlan,
    descriptor_digest: String,
    native_artifact_digest: String,
    build_inputs: BTreeMap<String, String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageLock {
    schema: String,
    version: u32,
    root: ManifestPlan,
    context: BTreeMap<String, String>,
    packages: BTreeMap<String, LockedPackage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiEntry {
    identity: String,
    function: Option<FunctionRecord>,
    source_type: Option<SourceTypeRecord>,
    source_template: Option<SourceTemplateRecord>,
    source_value: Option<String>,
    native_type: Option<TypeRecord>,
    constant: Option<(String, String)>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageApi {
    schema: String,
    version: u32,
    package: String,
    language: String,
    target: String,
    semantics: String,
    exports: BTreeMap<String, ApiEntry>,
}

#[derive(Debug, Serialize)]
struct ApiReport {
    schema: &'static str,
    version: u32,
    package: String,
    breaking_candidates: Vec<String>,
    additions: Vec<String>,
    behavior_equivalence: bool,
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn relative(root: &Path, path: &Path) -> PathBuf {
    let root: Vec<_> = root.components().collect();
    let path: Vec<_> = path.components().collect();
    let common = root.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in common..root.len() {
        result.push("..");
    }
    for part in &path[common..] {
        result.push(part.as_os_str());
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

fn snapshot(
    root: &Manifest,
    inputs: &NativeCliInputs,
    packages: &[ResolvedSdkPackage],
    built: &crate::native_package::BuiltNativePackages,
    own: Option<(
        &ResolvedSdkPackage,
        &crate::source_native_package::BuiltSourcePackage,
    )>,
) -> Result<PackageLock, String> {
    let mut context = crate::native_clang::TargetClang::resolve()?.fingerprint;
    let mut compilation = built.context_inputs.clone();
    // A dependency lock is not an application-source lock. SDK descriptors and
    // root native interfaces retain their checked source identity independently.
    if root.kind == PackageKind::Application
        && own.is_none()
        && let Some(wire) = compilation.get(rinzu_bindgen::compilation::KEY)
    {
        let mut record: serde_json::Value =
            serde_json::from_str(wire).map_err(|error| error.to_string())?;
        record["purpose"] = "dependency-lock".into();
        record["graph"] = serde_json::json!([]);
        record["checked"] = serde_json::json!({});
        rinzu_bindgen::compilation::encode(&mut compilation, record)?;
    }
    context.extend(compilation);
    for (key, value) in &built.explicit_link_inputs {
        context.insert(format!("explicit.{key}"), value.clone());
    }
    context.insert(
        "rinzu.semantics".into(),
        rinzu_compiler::LANGUAGE_SEMANTICS_REVISION.into(),
    );
    context.insert(
        "rinzu.private-abi".into(),
        rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION.into(),
    );
    context.insert(
        "rinzu.compiler".into(),
        sha256(
            &std::fs::read(std::env::current_exe().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?,
        ),
    );
    context.insert(
        "native.search-paths".into(),
        serde_json::to_string(&inputs.search_paths).map_err(|e| e.to_string())?,
    );
    context.insert(
        "native.libraries".into(),
        serde_json::to_string(&inputs.libraries).map_err(|e| e.to_string())?,
    );
    for object in &inputs.objects {
        context.insert(
            format!("native.object:{}", object.display()),
            sha256(&std::fs::read(root.root.join(object)).map_err(|e| e.to_string())?),
        );
    }
    let mut locked = BTreeMap::new();
    for (package, built) in packages.iter().zip(&built.source_packages).chain(own) {
        let descriptor = validate_source_descriptor(&built.artifact.expected_wire)?;
        if descriptor.interface.package.package != package.manifest.name {
            return Err("source graph package order/identity mismatch".into());
        }
        let record = LockedPackage {
            path: relative(&root.root, &package.manifest.root),
            manifest: plan(&package.manifest),
            descriptor_digest: sha256(&built.artifact.expected_wire),
            native_artifact_digest: descriptor.native_artifact_digest,
            build_inputs: descriptor.interface.package.build_inputs,
        };
        if locked
            .insert(package.manifest.name.clone(), record)
            .is_some()
        {
            return Err("duplicate source graph identity".into());
        }
    }
    if packages.len() != built.source_packages.len() {
        return Err("incomplete source graph".into());
    }
    Ok(PackageLock {
        schema: LOCK_SCHEMA.into(),
        version: VERSION,
        root: plan(root),
        context,
        packages: locked,
    })
}

fn verify(root: &Path, expected: &PackageLock) -> Result<(), String> {
    let path = root.join("rinzu.lock");
    let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let actual: PackageLock =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid package lock: {e}"))?;
    if actual.schema != LOCK_SCHEMA || actual.version != VERSION {
        return Err("unsupported package lock schema/version; regenerate with `rinzu lock`".into());
    }
    if actual != *expected {
        let mut changed = Vec::new();
        if actual.root != expected.root {
            changed.push("root manifest".to_owned());
        }
        if actual.context != expected.context {
            changed.push("target/toolchain/native context".to_owned());
        }
        for name in actual.packages.keys().chain(expected.packages.keys()) {
            if actual.packages.get(name) != expected.packages.get(name) && !changed.contains(name) {
                changed.push(name.clone());
            }
        }
        return Err(format!(
            "package lock mismatch ({}); review changes and run `rinzu lock`",
            changed.join(", ")
        ));
    }
    Ok(())
}

fn has_lock(root: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(root.join("rinzu.lock")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("inspect package lock: {error}")),
    }
}

pub fn check_application_lock(
    root: &Path,
    inputs: &NativeCliInputs,
    packages: &[ResolvedSdkPackage],
    built: &crate::native_package::BuiltNativePackages,
) -> Result<(), String> {
    if !has_lock(root)? {
        return Ok(());
    }
    let manifest = rinzu_compiler::load_manifest(&root.join("rinzu.toml"))
        .map_err(|e| format!("{:?}", e.into_diagnostic()))?;
    if manifest.kind == PackageKind::Application {
        verify(root, &snapshot(&manifest, inputs, packages, built, None)?)?;
    }
    Ok(())
}

pub fn check_sdk_lock(
    package: &ResolvedSdkPackage,
    inputs: &NativeCliInputs,
    packages: &[ResolvedSdkPackage],
    dependencies: &crate::native_package::BuiltNativePackages,
    built: &crate::source_native_package::BuiltSourcePackage,
) -> Result<(), String> {
    let manifest = &package.manifest;
    if has_lock(&manifest.root)? {
        verify(
            &manifest.root,
            &snapshot(
                manifest,
                inputs,
                packages,
                dependencies,
                Some((package, built)),
            )?,
        )?;
    }
    Ok(())
}

fn fresh(
    root: &Manifest,
    artifact: &FrontendArtifact,
) -> Result<(PackageLock, Option<SourceDescriptor>), String> {
    let mut input_snapshot = crate::source_receipt::InputSnapshot::checked(
        &artifact.checked_inputs,
        &artifact.module_graph,
    )?;
    input_snapshot.native(root, &artifact.native_inputs)?;
    let mut built = crate::native_package::build_unlocked(
        &artifact.sdk_packages,
        &root.root,
        &artifact.native_inputs,
    )?;
    let _temporary = Temporary(built.directory.clone());
    let own = if let Some(source) = &artifact.source_interface {
        let package = ResolvedSdkPackage {
            manifest: root.clone(),
            native_imports: Vec::new(),
            source: Some(source.clone()),
        };
        let directory = built.directory.join("root");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let sdk = crate::source_native_package::build_with_dependencies(
            &package,
            &root.root,
            &artifact.native_inputs,
            &directory,
            &built.dependency_digests,
        )?;
        Some((package, sdk))
    } else {
        None
    };
    built.context(root, artifact, own.as_ref().map(|(_, package)| package))?;
    let sysroot = built
        .context_inputs
        .get("clang.sysroot")
        .map_or(Path::new(""), |path| Path::new(path));
    input_snapshot.recorded(&root.root, sysroot, &built.context_inputs)?;
    input_snapshot.verify()?;
    let lock = snapshot(
        root,
        &artifact.native_inputs,
        &artifact.sdk_packages,
        &built,
        own.as_ref().map(|(p, b)| (p, b)),
    )?;
    let descriptor = own
        .as_ref()
        .map(|(_, b)| validate_source_descriptor(&b.artifact.expected_wire))
        .transpose()?;
    Ok((lock, descriptor))
}

fn api(descriptor: &SourceDescriptor) -> Result<PackageApi, String> {
    let interface = &descriptor.interface;
    let mut exports = BTreeMap::new();
    for (alias, identity) in &interface.public_bindings {
        let mut function = interface
            .functions
            .iter()
            .find(|f| &f.identity == identity)
            .cloned();
        if let Some(function) = &mut function {
            function.callback_sites.clear();
        }
        let entry = ApiEntry {
            identity: identity.clone(),
            function,
            source_type: interface.source_types.get(identity).cloned(),
            source_template: interface.source_templates.get(identity).cloned(),
            source_value: interface.source_values.get(identity).cloned(),
            native_type: interface
                .types
                .iter()
                .find(|t| &t.identity == identity)
                .cloned(),
            constant: interface
                .constants
                .get(identity)
                .map(|value| (interface.constant_types[identity].clone(), value.clone())),
        };
        if entry.function.is_none()
            && entry.source_type.is_none()
            && entry.source_template.is_none()
            && entry.source_value.is_none()
            && entry.native_type.is_none()
            && entry.constant.is_none()
        {
            return Err(format!(
                "public API `{alias}` has no checked schema for `{identity}`"
            ));
        }
        exports.insert(alias.clone(), entry);
    }
    Ok(PackageApi {
        schema: API_SCHEMA.into(),
        version: VERSION,
        package: interface.package.package.clone(),
        language: interface.package.language.clone(),
        target: interface.package.target.clone(),
        semantics: rinzu_compiler::LANGUAGE_SEMANTICS_REVISION.into(),
        exports,
    })
}

fn compare(previous: &PackageApi, current: &PackageApi) -> Result<ApiReport, String> {
    if previous.schema != API_SCHEMA || previous.version != VERSION {
        return Err("unsupported package API schema/version".into());
    }
    if previous.package != current.package {
        return Err("package API comparison requires the same package identity".into());
    }
    let mut breaking_candidates = Vec::new();
    if previous.language != current.language
        || previous.target != current.target
        || previous.semantics != current.semantics
    {
        breaking_candidates.push("language/target/semantics changed".into());
    }
    for (name, old) in &previous.exports {
        match current.exports.get(name) {
            None => breaking_candidates.push(format!("removed {name} ({})", old.identity)),
            Some(new) if new != old => {
                breaking_candidates.push(format!("changed {name} ({})", old.identity))
            }
            _ => {}
        }
    }
    let additions = current
        .exports
        .keys()
        .filter(|key| !previous.exports.contains_key(*key))
        .cloned()
        .collect();
    Ok(ApiReport {
        schema: "rinzu.package-api-report",
        version: VERSION,
        package: current.package.clone(),
        breaking_candidates,
        additions,
        behavior_equivalence: false,
    })
}

pub(crate) fn output_identity(path: &Path) -> Result<PathBuf, String> {
    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
    let name = path.file_name().ok_or("output path needs a file name")?;
    let mut parent = PathBuf::new();
    for component in path.parent().unwrap().components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parent.pop();
            }
            component => {
                parent.push(component.as_os_str());
                match std::fs::canonicalize(&parent) {
                    Ok(resolved) => parent = resolved,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
    }
    Ok(parent.join(name))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Replace the file only after the complete snapshot exists; do not follow an old file symlink.
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    use std::io::Write;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

pub fn run(command: &str, mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut check = false;
    let mut previous = None;
    let mut markdown = None;
    let mut ordinary = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--markdown" if command == "api" => {
                if markdown.is_some() {
                    return Err("duplicate --markdown option".into());
                }
                markdown = Some(PathBuf::from(
                    arguments
                        .next()
                        .filter(|p| !p.starts_with('-'))
                        .ok_or("--markdown requires a reference path")?,
                ))
            }
            "--check" if command == "lock" && !check => check = true,
            "--compare" if command == "api" && previous.is_none() => {
                previous = Some(PathBuf::from(
                    arguments
                        .next()
                        .filter(|p| !p.starts_with('-'))
                        .ok_or("--compare requires a snapshot path")?,
                ))
            }
            _ => ordinary.push(argument),
        }
    }
    let parsed = crate::native_cli::parse(
        if command == "api" { "build" } else { "lock" },
        ordinary.into_iter(),
    )?;
    let path = parsed
        .path
        .map_or_else(|| std::env::current_dir().map_err(|e| e.to_string()), Ok)?;
    crate::input::require_package(command, &path)?;
    let (manifest, artifact) =
        rinzu_compiler::check_package_with_native_inputs(&path, &parsed.native)
            .map_err(|e| format!("package failed to check: {e:?}"))?;
    if command == "api" && manifest.kind != PackageKind::Sdk {
        return Err("`rinzu api` requires a source SDK/library package".into());
    }
    let (lock, descriptor) = fresh(&manifest, &artifact)?;
    if command == "lock" {
        let path = manifest.root.join("rinzu.lock");
        if check {
            verify(&manifest.root, &lock)?;
            println!("checked lock: {}", path.display());
        } else {
            write_json(&path, &lock)?;
            println!("lock: {}", path.display());
        }
    } else {
        let current = api(&descriptor.ok_or("source SDK descriptor is missing")?)?;
        // Read the baseline before replacing output, including when both paths are identical.
        let report = previous
            .map(|path| -> Result<_, String> {
                let old: PackageApi =
                    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
                        .map_err(|e| format!("invalid package API: {e}"))?;
                compare(&old, &current)
            })
            .transpose()?;
        let output = parsed
            .output
            .unwrap_or_else(|| manifest.root.join("build/package-api-v2.json"));
        let rendered = markdown
            .as_ref()
            .map(|path| {
                let identity = output_identity(path)?;
                if identity == output_identity(&output)?
                    || report.is_some()
                        && identity == output_identity(&output.with_extension("report.json"))?
                {
                    return Err(
                        "Markdown, API snapshot and comparison report outputs must differ".into(),
                    );
                }
                documentation::render(&current, &manifest, &artifact, path, &output)
            })
            .transpose()?;
        write_json(&output, &current)?;
        if let Some(report) = report {
            let path = output.with_extension("report.json");
            write_json(&path, &report)?;
            println!("report: {}", path.display());
        }
        if let (Some(path), Some(rendered)) = (markdown, rendered) {
            write_bytes(&path, rendered.as_bytes())?;
            println!("API reference: {}", path.display());
        }
        println!("API snapshot: {}", output.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_are_closed_versioned_and_api_diffs_are_conservative() {
        let value = serde_json::json!({
            "schema": API_SCHEMA, "version": VERSION, "package": "model", "language": "rinzu-0.5",
            "target": "aarch64-apple-darwin", "semantics": "rinzu-language-0.5-r2", "exports": {}
        });
        let original: PackageApi = serde_json::from_value(value.clone()).unwrap();
        let mut unknown = value;
        unknown["unknown"] = true.into();
        assert!(serde_json::from_value::<PackageApi>(unknown).is_err());
        let mut current = PackageApi {
            exports: BTreeMap::new(),
            ..serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap()
        };
        current.exports.insert(
            "model.value".into(),
            ApiEntry {
                identity: "value".into(),
                function: None,
                source_type: None,
                source_template: None,
                source_value: Some("String".into()),
                native_type: None,
                constant: None,
            },
        );
        let additions = compare(&original, &current).unwrap();
        assert_eq!(additions.additions, ["model.value"]);
        assert!(additions.breaking_candidates.is_empty());
        assert!(!additions.behavior_equivalence);
        assert_eq!(
            compare(&current, &original).unwrap().breaking_candidates,
            ["removed model.value (value)"]
        );
        current.version = 1;
        assert!(compare(&current, &original).is_err());
    }

    #[test]
    fn lock_rejects_old_unknown_target_toolchain_and_dangling_files() {
        let directory =
            std::env::temp_dir().join(format!("rinzu-lock-unit-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let value = serde_json::json!({
            "schema":LOCK_SCHEMA,"version":VERSION,
            "root":{"name":"app","language":"0.5","kind":"application","entry":"src/main.rinzu","exports":[],"dependencies":{},"native":{}},
            "context":{"clang.target":"arm64-apple-macos11","clang.binary":"digest","target.os-build":"build","rinzu.private-abi":"revision"},"packages":{}
        });
        let expected: PackageLock = serde_json::from_value(value.clone()).unwrap();
        let path = directory.join("rinzu.lock");
        write_json(&path, &expected).unwrap();
        verify(&directory, &expected).unwrap();
        for key in [
            "clang.target",
            "clang.binary",
            "target.os-build",
            "rinzu.private-abi",
        ] {
            let mut stale = value.clone();
            stale["context"][key] = "changed".into();
            std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
            assert!(
                verify(&directory, &expected)
                    .unwrap_err()
                    .contains("mismatch")
            );
        }
        for (key, changed) in [
            ("version", serde_json::json!(1)),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut stale = value.clone();
            stale[key] = changed;
            std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
            assert!(verify(&directory, &expected).is_err());
        }
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(directory.join("missing"), &path).unwrap();
        assert!(has_lock(&directory).unwrap());
        assert!(verify(&directory, &expected).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
