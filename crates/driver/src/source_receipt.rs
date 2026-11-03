//! Checked source resolution, owner scopes and post-build byte verification.
use rinzu_bindgen::source_package::sha256;
use rinzu_compiler::{Manifest, NativeCliInputs, ResolvedSourceModule};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct InputSnapshot {
    files: BTreeMap<PathBuf, (PathBuf, u64, u64, String)>,
    resolutions: BTreeMap<PathBuf, PathBuf>,
}

impl InputSnapshot {
    /// Check one union of the expected snapshots. Shared inputs are read once
    /// in this verification operation, with no metadata-only or persistent memo.
    pub fn verify_all<'a>(snapshots: impl IntoIterator<Item = &'a Self>) -> Result<(), String> {
        let _timing = rinzu_compiler::timing::span("native.inputs.verify_group");
        let mut combined = Self::default();
        for snapshot in snapshots {
            for (path, expected) in &snapshot.files {
                if combined
                    .files
                    .get(path)
                    .is_some_and(|value| value != expected)
                {
                    return Err(format!(
                        "input changed before publication: {}; check again",
                        path.display()
                    ));
                }
                combined.files.insert(path.clone(), expected.clone());
            }
            for (path, target) in &snapshot.resolutions {
                if combined
                    .resolutions
                    .get(path)
                    .is_some_and(|value| value != target)
                {
                    return Err(format!(
                        "module resolution changed after checking: {}; check again",
                        path.display()
                    ));
                }
                combined.resolutions.insert(path.clone(), target.clone());
            }
        }
        combined.verify()
    }

    pub fn checked(
        bytes: &BTreeMap<PathBuf, Vec<u8>>,
        graph: &[ResolvedSourceModule],
    ) -> Result<Self, String> {
        let _timing = rinzu_compiler::timing::span("native.inputs.capture.checked");
        let mut snapshot = Self::default();
        for (path, bytes) in bytes {
            snapshot.add(path, Some(&sha256(bytes)))?;
        }
        for node in graph {
            snapshot
                .resolutions
                .insert(node.path.clone(), node.path.clone());
            for import in &node.imports {
                if let (Some(requested), Some(target)) =
                    (&import.requested_path, &import.target_path)
                {
                    snapshot
                        .resolutions
                        .insert(requested.clone(), target.clone());
                }
            }
        }
        snapshot.verify()?;
        Ok(snapshot)
    }

    pub fn add(&mut self, path: &Path, expected: Option<&str>) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("resolve checked input {}: {error}", path.display()))?;
        let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err(format!("checked input is not a file: {}", path.display()));
        }
        let digest = sha256(&std::fs::read(&canonical).map_err(|error| error.to_string())?);
        if expected.is_some_and(|expected| digest != expected) {
            return Err(format!(
                "input changed after checking: {}; check again",
                path.display()
            ));
        }
        let value = (canonical, metadata.dev(), metadata.ino(), digest);
        if self
            .files
            .get(path)
            .is_some_and(|previous| previous != &value)
        {
            return Err(format!(
                "input changed during native preparation: {}",
                path.display()
            ));
        }
        self.files.insert(path.into(), value);
        Ok(())
    }

    pub fn native(&mut self, manifest: &Manifest, cli: &NativeCliInputs) -> Result<(), String> {
        for path in manifest
            .native
            .sources
            .iter()
            .chain(&manifest.native.headers)
            .chain(&manifest.native.objects)
        {
            self.add(&manifest.root.join(path), None)?;
        }
        for path in cli.objects.iter().chain(&cli.headers) {
            self.add(&manifest.root.join(path), None)?;
        }
        Ok(())
    }

    pub fn recorded(
        &mut self,
        root: &Path,
        sysroot: &Path,
        inputs: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let _timing = rinzu_compiler::timing::span("native.inputs.capture.recorded");
        if let Some(digest) = inputs.get("clang.sdk") {
            self.add(&sysroot.join("SDKSettings.json"), Some(digest))?;
        }
        if let Some(digest) = inputs.get("rinzu.compiler-binary") {
            self.add(
                &std::env::current_exe().map_err(|error| error.to_string())?,
                Some(digest),
            )?;
        }
        if let Some(root) = inputs.get("rinzu.toolchain-resource-root") {
            let root = Path::new(root);
            rinzu_compiler::standard_library::verify(root)?;
            self.add(
                &root.join("registry.json"),
                Some(&rinzu_compiler::standard_library::sha256(
                    &rinzu_compiler::standard_library::registry_bytes(),
                )),
            )?;
            for (relative, bytes) in rinzu_compiler::standard_library::resources() {
                if relative.starts_with("include/")
                    || relative.starts_with("runtime/")
                    || relative == "license-inventory.json"
                {
                    self.add(
                        &root.join(relative),
                        Some(&rinzu_compiler::standard_library::sha256(bytes)),
                    )?;
                }
            }
        }
        for (key, name) in [
            ("rinzu.abi-header", "runtime.h"),
            ("rinzu.completion-header", "native_completion.h"),
        ] {
            if let Some(digest) = inputs.get(key) {
                self.add(
                    &rinzu_compiler::standard_library::resource_root()?
                        .join("include/rinzu")
                        .join(name),
                    Some(digest),
                )?;
            }
        }
        if let (Some(path), Some(digest)) =
            (inputs.get("clang.identity"), inputs.get("clang.binary"))
        {
            self.add(Path::new(path), Some(digest))?;
        }
        if let (Some(path), Some(digest)) = (
            inputs.get("config.tool"),
            inputs.get("config.identity-version-digest"),
        ) {
            self.add(Path::new(path), Some(digest))?;
        }
        for (key, path) in inputs {
            if let Some(name) = key.strip_prefix("tool.identity:") {
                self.add(
                    Path::new(path),
                    inputs
                        .get(&format!("tool.binary:{name}"))
                        .map(String::as_str),
                )?;
            }
        }
        for (key, digest) in inputs {
            let path = ["link:", "include:", "header:"]
                .iter()
                .find_map(|prefix| key.strip_prefix(prefix))
                .and_then(|path| {
                    if let Some(path) = path.strip_prefix("sdk:") {
                        Some(root.join(path))
                    } else if let Some(path) = path.strip_prefix("sysroot:") {
                        Some(sysroot.join(path))
                    } else if let Some(path) = path.strip_prefix("external:") {
                        Some(PathBuf::from(path))
                    } else if Path::new(path).is_absolute() {
                        Some(PathBuf::from(path))
                    } else {
                        None
                    }
                });
            if let Some(path) = path {
                self.add(&path, Some(digest))?;
            }
            if let Some(path) = key.strip_prefix("native.file:") {
                self.add(Path::new(path), Some(digest))?;
            }
        }
        Ok(())
    }

    pub fn verify(&self) -> Result<(), String> {
        let _timing = rinzu_compiler::timing::span("native.inputs.verify");
        use std::os::unix::fs::MetadataExt;
        for (path, (canonical, dev, ino, digest)) in &self.files {
            let current = path.canonicalize().map_err(|error| {
                format!("checked input disappeared {}: {error}", path.display())
            })?;
            let metadata = std::fs::metadata(&current).map_err(|error| error.to_string())?;
            if current != *canonical
                || metadata.dev() != *dev
                || metadata.ino() != *ino
                || sha256(&std::fs::read(&current).map_err(|error| error.to_string())?) != *digest
            {
                return Err(format!(
                    "input changed before publication: {}; check again",
                    path.display()
                ));
            }
        }
        for (requested, target) in &self.resolutions {
            if requested
                .canonicalize()
                .map_err(|error| error.to_string())?
                != *target
            {
                return Err(format!(
                    "module resolution changed after checking: {}; check again",
                    requested.display()
                ));
            }
        }
        Ok(())
    }
}

fn identity(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).map_or_else(
        |_| path.display().to_string(),
        |path| format!("owner:{}", path.display()),
    )
}

pub fn fingerprint(
    manifest: &Manifest,
    mode: &str,
    graph: &[ResolvedSourceModule],
    checked: &BTreeMap<PathBuf, Vec<u8>>,
    cli: &NativeCliInputs,
    inputs: &mut BTreeMap<String, String>,
) -> Result<String, String> {
    let _timing = rinzu_compiler::timing::span("native.inputs.fingerprint");
    let root = &manifest.root;
    let mut nodes = BTreeMap::new();
    for node in graph {
        let bytes = checked
            .get(&node.path)
            .ok_or_else(|| format!("checked module bytes are missing: {}", node.path.display()))?;
        let imports: Vec<_> = node.imports.iter().map(|import| serde_json::json!({
            "kind": import.kind, "qualifier": import.qualifier, "target": import.target,
            "requested": import.requested_path.as_ref().map(|path| identity(root, path)),
            "path": import.target_path.as_ref().map(|path| identity(root, path)), "public": import.public_symbols,
        })).collect();
        nodes.insert(
            identity(root, &node.path),
            serde_json::json!({
                "path": identity(root, &node.path), "key": node.key, "entry": node.is_entry,
                "sha256": sha256(bytes), "imports": imports, "exports": node.exports,
            }),
        );
    }
    let paths = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|path| identity(root, &root.join(path)))
            .collect::<Vec<_>>()
    };
    let scope = serde_json::json!({
        "owner": manifest.name, "kind": format!("{:?}", manifest.kind),
        "manifest": {"sources": paths(&manifest.native.sources), "objects": paths(&manifest.native.objects),
            "headers": paths(&manifest.native.headers), "includeDirs": paths(&manifest.native.include_dirs),
            "searchPaths": paths(&manifest.native.search_paths), "libraries": manifest.native.libraries,
            "frameworks": manifest.native.frameworks, "configTools": manifest.native.config_tools},
        "cli": {"objects": paths(&cli.objects), "headers": paths(&cli.headers),
            "searchPaths": paths(&cli.search_paths), "libraries": cli.libraries},
    });
    let mut scope_files = BTreeMap::new();
    for path in manifest
        .native
        .sources
        .iter()
        .chain(&manifest.native.headers)
        .chain(&manifest.native.objects)
        .chain(&cli.objects)
        .chain(&cli.headers)
    {
        let path = root.join(path);
        scope_files.insert(
            identity(root, &path),
            sha256(&std::fs::read(path).map_err(|error| error.to_string())?),
        );
    }
    let mut closure = inputs.clone();
    closure.remove(rinzu_bindgen::compilation::KEY);
    closure.remove(rinzu_bindgen::compilation::DIGEST_KEY);
    // Native contracts may have already added their headers to checked_inputs.
    // They belong to scopeFiles in either entry path; recording them a second
    // time only for `check_input` makes a freshly locked SDK fail `build`.
    let checked_sources: BTreeMap<_, _> = checked
        .iter()
        .map(|(path, bytes)| (identity(root, path), sha256(bytes)))
        .filter(|(path, _)| !scope_files.contains_key(path))
        .collect();
    rinzu_bindgen::compilation::encode(
        inputs,
        serde_json::json!({
            "schema": rinzu_bindgen::compilation::SCHEMA, "mode": mode, "owner": manifest.name,
            "entry": identity(root, &root.join(&manifest.entry)), "language": manifest.language,
            "semantics": rinzu_compiler::LANGUAGE_SEMANTICS_REVISION,
            "privateRuntime": rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION,
            "sourcePackage": [rinzu_abi::SOURCE_PACKAGE_MAJOR, rinzu_abi::SOURCE_PACKAGE_MINOR],
            "nativeDataContract": rinzu_abi::NATIVE_DATA_CONTRACT_VERSION,
            "manifest": {"exports": manifest.exports.modules, "dependencies": manifest.dependencies.iter().map(|dependency| (&dependency.name, &dependency.path)).collect::<Vec<_>>()},
        "checked": checked_sources,
        "scopeFiles": scope_files,
            "target": crate::native_clang::TARGET, "graph": nodes.into_values().collect::<Vec<_>>(),
            "scope": scope, "inputs": closure,
        }),
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    schema: String,
    input_fingerprint: String,
    files: BTreeMap<String, String>,
}

pub fn seal(
    directory: &Path,
    inputs: &BTreeMap<String, String>,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), String> {
    let _timing = rinzu_compiler::timing::span("native.artifact.seal");
    let receipt = ArtifactReceipt {
        schema: "rinzu.compilation-artifact-v1".into(),
        input_fingerprint: inputs
            .get(rinzu_bindgen::compilation::DIGEST_KEY)
            .ok_or("missing compilation input fingerprint")?
            .clone(),
        files: files
            .iter()
            .map(|(path, bytes)| (path.clone(), sha256(bytes)))
            .collect(),
    };
    std::fs::write(
        directory.join("source-input-receipt.json"),
        serde_json::to_vec(&receipt).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub fn verify_seal(directory: &Path, inputs: &BTreeMap<String, String>) -> Result<(), String> {
    let _timing = rinzu_compiler::timing::span("native.artifact.verify");
    let Some(expected) = inputs.get(rinzu_bindgen::compilation::DIGEST_KEY) else {
        return Ok(());
    };
    let receipt_path = directory.join("source-input-receipt.json");
    if !std::fs::symlink_metadata(&receipt_path)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_file()
    {
        return Err("source artifact receipt is not a regular file".into());
    }
    let receipt: ArtifactReceipt = serde_json::from_slice(
        &std::fs::read(directory.join("source-input-receipt.json"))
            .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    if receipt.schema != "rinzu.compilation-artifact-v1"
        || &receipt.input_fingerprint != expected
        || !receipt.files.contains_key("source-package-v2.json")
    {
        return Err("source artifact receipt mismatch".into());
    }
    for (name, digest) in &receipt.files {
        if Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err("source artifact receipt path is not confined".into());
        }
        let path = directory.join(name);
        if std::fs::symlink_metadata(&path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
            || sha256(&std::fs::read(&path).map_err(|error| error.to_string())?) != *digest
        {
            return Err(format!("source artifact receipt file mismatch: {name}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinzu_compiler::{CompilationContext, CompilationOptions};

    fn formatted(source: &str) -> String {
        rinzu_compiler::format_source(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(1),
            "fixture.rinzu",
            source,
        ))
        .unwrap()
    }

    fn fixture(name: &str) -> crate::verify_capture::TemporaryDirectory {
        let root =
            std::env::temp_dir().join(format!("rinzu-receipt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (name, source) in [
            (
                "main.rinzu",
                "import \"./alias.rinzu\" as helper; fn main() { let value = helper.answer(); }",
            ),
            (
                "one.rinzu",
                "fn answer() -> Int { return 42; } export answer;",
            ),
            (
                "two.rinzu",
                "fn answer() -> Int { return 42; } export answer;",
            ),
        ] {
            std::fs::write(root.join(name), formatted(source)).unwrap();
        }
        std::os::unix::fs::symlink("one.rinzu", root.join("alias.rinzu")).unwrap();
        crate::verify_capture::TemporaryDirectory(root)
    }

    fn checked(root: &Path) -> CompilationContext {
        rinzu_compiler::check_input(
            &root.join("main.rinzu"),
            &CompilationOptions {
                language: None,
                target: rinzu_runtime::current_target().into(),
            },
            &NativeCliInputs::default(),
        )
        .unwrap()
    }

    fn digest(
        context: &CompilationContext,
        cli: &NativeCliInputs,
        mut inputs: BTreeMap<String, String>,
    ) -> String {
        fingerprint(
            &context.metadata,
            "File",
            &context.artifact.module_graph,
            &context.artifact.checked_inputs,
            cli,
            &mut inputs,
        )
        .unwrap()
    }

    #[test]
    fn merged_snapshots_reject_conflicting_expected_bytes() {
        let temporary = fixture("snapshot-conflict");
        let path = temporary.0.join("one.rinzu");
        let mut before = InputSnapshot::default();
        before.add(&path, None).unwrap();
        std::fs::write(&path, "changed").unwrap();
        let mut after = InputSnapshot::default();
        after.add(&path, None).unwrap();
        assert!(InputSnapshot::verify_all([&after, &before]).is_err());
        assert!(InputSnapshot::verify_all([&before, &after]).is_err());
    }

    #[test]
    fn merged_snapshots_freshly_check_shared_bytes_and_resolution() {
        let temporary = fixture("snapshot-shared");
        let path = temporary.0.join("one.rinzu");
        let mut snapshot = InputSnapshot::default();
        snapshot.add(&path, None).unwrap();
        let alias = temporary.0.join("alias.rinzu");
        snapshot
            .resolutions
            .insert(alias.clone(), path.canonicalize().unwrap());
        InputSnapshot::verify_all([&snapshot, &snapshot]).unwrap();
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink("two.rinzu", &alias).unwrap();
        assert!(InputSnapshot::verify_all([&snapshot, &snapshot]).is_err());
        snapshot.resolutions.clear();
        std::fs::write(&path, "changed").unwrap();
        assert!(InputSnapshot::verify_all([&snapshot, &snapshot]).is_err());
    }

    #[test]
    fn checked_native_headers_have_one_stable_identity_and_still_invalidate_inputs() {
        let temporary = fixture("native-header");
        let mut context = checked(&temporary.0);
        let header = context.metadata.root.join("api.h");
        std::fs::write(&header, b"#pragma once\n").unwrap();
        let cli = NativeCliInputs {
            headers: vec![PathBuf::from("api.h")],
            ..Default::default()
        };
        let before = digest(&context, &cli, BTreeMap::new());
        context
            .artifact
            .checked_inputs
            .insert(header.clone(), std::fs::read(&header).unwrap());
        assert_eq!(before, digest(&context, &cli, BTreeMap::new()));
        std::fs::write(&header, b"#pragma once\nint changed(void);\n").unwrap();
        assert_ne!(before, digest(&context, &cli, BTreeMap::new()));
    }

    #[test]
    fn checked_source_and_same_byte_symlink_retarget_require_a_fresh_graph() {
        let temporary = fixture("graph");
        let context = checked(&temporary.0);
        let snapshot = InputSnapshot::checked(
            &context.artifact.checked_inputs,
            &context.artifact.module_graph,
        )
        .unwrap();
        let original = digest(&context, &NativeCliInputs::default(), BTreeMap::new());
        std::fs::remove_file(temporary.0.join("alias.rinzu")).unwrap();
        std::os::unix::fs::symlink("two.rinzu", temporary.0.join("alias.rinzu")).unwrap();
        assert!(
            snapshot
                .verify()
                .unwrap_err()
                .contains("module resolution changed")
        );
        let fresh = checked(&temporary.0);
        assert_ne!(
            original,
            digest(&fresh, &NativeCliInputs::default(), BTreeMap::new())
        );
        std::fs::write(
            temporary.0.join("two.rinzu"),
            formatted("fn answer() -> Int { return 43; } export answer;"),
        )
        .unwrap();
        assert!(
            InputSnapshot::checked(&fresh.artifact.checked_inputs, &fresh.artifact.module_graph)
                .err()
                .unwrap()
                .contains("changed after checking")
        );
    }

    #[test]
    fn generated_unit_entry_keeps_labels_and_only_guards_physical_sources() {
        let temporary = fixture("unit-graph");
        std::fs::create_dir(temporary.0.join("src")).unwrap();
        std::fs::write(
            temporary.0.join("rinzu.toml"),
            "[package]\nname=\"unit_receipt\"\nlanguage=\"0.5\"\n",
        )
        .unwrap();
        let entry = temporary.0.join("src/main.rinzu");
        std::fs::write(&entry, formatted("fn test_value() {}")).unwrap();
        let (_, artifact, _) = rinzu_compiler::check_package_tests(
            &temporary.0,
            &mut rinzu_compiler::SourceSnapshot::new(
                &mut rinzu_compiler::SourceCache::default(),
                Default::default(),
            ),
            &rinzu_compiler::PackageTestOptions {
                unit: true,
                selected: Some("unit:main::test_value".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let snapshot =
            InputSnapshot::checked(&artifact.checked_inputs, &artifact.module_graph).unwrap();
        assert!(
            artifact
                .ir
                .source_files
                .values()
                .any(|path| path.ends_with("/<test entry>"))
        );
        std::fs::remove_file(entry).unwrap();
        assert!(
            snapshot
                .verify()
                .unwrap_err()
                .contains("checked input disappeared")
        );
    }

    #[test]
    fn scope_tool_target_and_graph_are_framed_without_file_id_or_order_noise() {
        let temporary = fixture("identity");
        let context = checked(&temporary.0);
        let original = digest(&context, &NativeCliInputs::default(), BTreeMap::new());
        let mut same = checked(&temporary.0);
        same.artifact.module_graph.reverse();
        assert_eq!(
            original,
            digest(&same, &NativeCliInputs::default(), BTreeMap::new())
        );
        for cli in [
            NativeCliInputs {
                search_paths: vec![temporary.0.clone()],
                ..Default::default()
            },
            NativeCliInputs {
                libraries: vec!["System".into()],
                ..Default::default()
            },
        ] {
            assert_ne!(original, digest(&context, &cli, BTreeMap::new()));
        }
        for key in ["clang.target", "tool.binary:ld", "dependency:api"] {
            assert_ne!(
                original,
                digest(
                    &context,
                    &NativeCliInputs::default(),
                    BTreeMap::from([(key.into(), "changed".into())])
                )
            );
        }
    }

    #[test]
    fn native_content_and_physical_replacement_and_generated_seal_are_checked() {
        let temporary = fixture("native");
        let path = temporary.0.join("input.o");
        std::fs::write(&path, b"native bytes").unwrap();
        let mut snapshot = InputSnapshot::default();
        snapshot.add(&path, None).unwrap();
        let saved = temporary.0.join("saved.o");
        std::fs::rename(&path, saved).unwrap();
        std::fs::write(&path, b"native bytes").unwrap();
        assert!(snapshot.verify().is_err());
        let mut snapshot = InputSnapshot::default();
        snapshot.add(&path, None).unwrap();
        std::fs::write(&path, b"changed bytes").unwrap();
        assert!(snapshot.verify().is_err());
        let inputs = BTreeMap::from([(
            rinzu_bindgen::compilation::DIGEST_KEY.into(),
            "a".repeat(64),
        )]);
        let files = BTreeMap::from([("source-package-v2.json".into(), b"wire".to_vec())]);
        std::fs::write(temporary.0.join("source-package-v2.json"), b"wire").unwrap();
        seal(&temporary.0, &inputs, &files).unwrap();
        verify_seal(&temporary.0, &inputs).unwrap();
        std::fs::write(temporary.0.join("source-package-v2.json"), b"tampered").unwrap();
        assert!(verify_seal(&temporary.0, &inputs).is_err());
    }

    #[test]
    fn production_file_receipt_rejects_checked_source_changes_before_load() {
        let temporary = fixture("publication");
        let context = checked(&temporary.0);
        let built = crate::input::build_native(&context).unwrap();
        let _scratch = crate::verify_capture::TemporaryDirectory(built.directory.clone());
        assert_eq!(
            built.context_inputs[rinzu_bindgen::compilation::DIGEST_KEY].len(),
            64
        );
        built.verify_inputs().unwrap();
        std::fs::write(
            temporary.0.join("one.rinzu"),
            formatted("fn answer() -> Int { return 43; } export answer;"),
        )
        .unwrap();
        assert!(
            built
                .load_source_packages()
                .err()
                .unwrap()
                .contains("changed before publication")
        );
    }

    #[test]
    fn production_library_selection_is_rechecked_before_load_and_bundle() {
        let temporary = fixture("selection");
        for name in ["high", "low", "pending"] {
            std::fs::create_dir(temporary.0.join(name)).unwrap();
        }
        let clang = crate::native_clang::TargetClang::resolve().unwrap();
        for (name, value) in [("low", 42), ("pending", 43)] {
            let source = temporary.0.join(format!("{name}.c"));
            std::fs::write(
                &source,
                format!("int rn_answer(void) {{ return {value}; }}\n"),
            )
            .unwrap();
            assert!(
                clang
                    .command()
                    .args(["-dynamiclib", "-install_name", "@rpath/libanswer.dylib"])
                    .arg(&source)
                    .arg("-o")
                    .arg(temporary.0.join(name).join("libanswer.dylib"))
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(temporary.0.join("main.rinzu"), formatted("native { fn answer() -> CInt symbol \"rn_answer\"; } fn main() { let value = answer(); }")).unwrap();
        let context = rinzu_compiler::check_input(
            &temporary.0.join("main.rinzu"),
            &CompilationOptions {
                language: None,
                target: rinzu_runtime::current_target().into(),
            },
            &NativeCliInputs {
                search_paths: vec![temporary.0.join("high"), temporary.0.join("low")],
                libraries: vec!["answer".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let built = crate::input::build_native(&context).unwrap();
        let _scratch = crate::verify_capture::TemporaryDirectory(built.directory.clone());
        built.verify_inputs().unwrap();
        std::fs::rename(
            temporary.0.join("pending/libanswer.dylib"),
            temporary.0.join("high/libanswer.dylib"),
        )
        .unwrap();
        assert!(
            built
                .load_source_packages()
                .err()
                .unwrap()
                .contains("native reference selection changed")
        );
        assert!(
            crate::source_package_bundle::save(&built.source_packages, &temporary.0.join("bundle"))
                .unwrap_err()
                .contains("native reference selection changed")
        );
    }
}
