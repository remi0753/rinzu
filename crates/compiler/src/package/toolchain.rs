use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::{
    ParsedModule, ResolvedSdkPackage, check_loaded, merge_packages, package_diagnostic,
    read_package_manifest,
};
use crate::compatibility::{LANGUAGE, RELEASE, version_parts};
use crate::official_sdk::{self, OfficialSdk};
use crate::standard_library::{self, sha256};
use crate::{
    Diagnostic, DiagnosticCode, FrontendArtifact, Manifest, PackageId, PackageKind,
    PackageLocation, PackageSource, PackageTestCase, PackageTestOptions, ResolvedGraph,
    SourceRange, SourceSnapshot,
};

type CheckedSdk = (ResolvedSdkPackage, Vec<ResolvedSdkPackage>);

/// The declared graph is validated against its portable lock before this pass.
/// Source imports extend it locally, in the same snapshot used to check each SDK.
pub(super) struct CheckGraph {
    graph: ResolvedGraph,
    names: HashMap<String, PackageId>,
    checked: HashMap<PackageId, CheckedSdk>,
    standard_root: Option<PathBuf>,
    official_root: Option<PathBuf>,
}

impl CheckGraph {
    pub(super) fn new(graph: ResolvedGraph) -> Self {
        let names = graph
            .packages()
            .filter(|(_, node)| node.manifest.kind == PackageKind::Sdk)
            .map(|(id, node)| (node.manifest.name.clone(), id))
            .collect();
        Self {
            graph,
            names,
            checked: HashMap::new(),
            standard_root: None,
            official_root: None,
        }
    }

    pub(super) fn manifest(&self, id: PackageId) -> &Manifest {
        &self.graph.package(id).manifest
    }

    pub(super) fn is_toolchain(&self, id: PackageId) -> bool {
        matches!(
            self.graph.package(id).location.source,
            PackageSource::Toolchain { .. }
        )
    }

    pub(super) fn check(
        mut self,
        sources: &mut SourceSnapshot<'_>,
        tests: Option<&PackageTestOptions>,
    ) -> Result<(Manifest, FrontendArtifact, Vec<PackageTestCase>), Vec<Diagnostic>> {
        for id in self.graph.take_order() {
            let children: Vec<_> = self
                .graph
                .package(id)
                .dependencies
                .values()
                .map(|child| &self.checked[child])
                .collect();
            let direct: Vec<_> = children
                .iter()
                .map(|(package, _)| package.clone())
                .collect();
            let mut dependencies = Vec::new();
            merge_packages(
                &mut dependencies,
                children.iter().flat_map(|(package, dependencies)| {
                    dependencies.iter().chain(std::iter::once(package)).cloned()
                }),
            )?;
            if id == ResolvedGraph::ROOT {
                return check_loaded(&mut self, id, "", dependencies, &direct, sources, tests);
            }
            let namespace = self.manifest(id).name.clone();
            let (manifest, artifact, _) = check_loaded(
                &mut self,
                id,
                &namespace,
                dependencies,
                &direct,
                sources,
                None,
            )?;
            self.remember(id, manifest, artifact);
        }
        unreachable!("the declared dependency order includes its root")
    }

    pub(super) fn imports(
        &mut self,
        owner: PackageId,
        modules: &[ParsedModule],
        sdk_test: bool,
        external: &mut HashMap<String, HashMap<String, String>>,
        sources: &mut SourceSnapshot<'_>,
    ) -> Result<Vec<ResolvedSdkPackage>, Vec<Diagnostic>> {
        let author = self.manifest(owner);
        let published = sources.is_archive(&author.root);
        let mut selected = BTreeMap::new();
        for module in modules {
            for import in module
                .syntax
                .imports
                .iter()
                .filter(|import| import.file.is_none())
            {
                let name = import.path.join(".");
                let extra = official_sdk::for_module(&name)
                    .map_err(|error| import_error(import.range, error))?;
                let package = if let Some(extra) = extra {
                    if external.contains_key(&name) {
                        // An explicit development dependency or this SDK's own test interface.
                        continue;
                    }
                    extra.name
                } else if let Some(package) = standard_library::source_package(&name) {
                    package
                } else {
                    continue;
                };
                if sdk_test && package == author.name {
                    continue;
                }
                selected
                    .entry(package.to_owned())
                    .or_insert((import.range, extra));
            }
        }
        let mut packages = Vec::new();
        for (name, (range, extra)) in selected {
            let minimum = extra.map_or(LANGUAGE, |sdk| &sdk.language);
            self.check_language(owner, &name, minimum, range)?;
            if published && extra.is_some_and(|sdk| !sdk.archive_compatible) {
                return Err(archive_error(range, &name));
            }
            let id = self.resolve(owner, &name, extra, range, sources)?;
            let (package, dependencies) = &self.checked[&id];
            if published
                && let Some(native) = dependencies
                    .iter()
                    .find(|dependency| !official_sdk::archive_compatible(&dependency.manifest.name))
            {
                return Err(archive_error(range, &native.manifest.name));
            }
            if let Some(source) = &package.source {
                for (module, members) in &source.modules {
                    let public = external.entry(module.clone()).or_default();
                    for (name, id) in members {
                        if public
                            .insert(name.clone(), id.clone())
                            .is_some_and(|old| old != *id)
                        {
                            return Err(import_error(
                                range,
                                format!("duplicate public member `{module}.{name}`"),
                            ));
                        }
                    }
                }
            }
            merge_packages(
                &mut packages,
                dependencies
                    .iter()
                    .cloned()
                    .chain(std::iter::once(package.clone())),
            )?;
        }
        Ok(packages)
    }

    fn check_language(
        &self,
        owner: PackageId,
        sdk: &str,
        minimum: &str,
        range: SourceRange,
    ) -> Result<(), Vec<Diagnostic>> {
        let required = version_parts::<2>(minimum);
        for id in [owner, ResolvedGraph::ROOT] {
            let requested = &self.manifest(id).language;
            if version_parts::<2>(requested) < required {
                return Err(vec![Diagnostic::error(
                    DiagnosticCode::InvalidModulePath,
                    range,
                    format!(
                        "toolchain package `{sdk}` requires language {minimum} (requested {requested}; {})",
                        self.graph.dependency_path(id),
                    ),
                    "set the package's language to a supported level that meets the SDK requirement",
                )]);
            }
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        owner: PackageId,
        name: &str,
        extra: Option<&OfficialSdk>,
        range: SourceRange,
        sources: &mut SourceSnapshot<'_>,
    ) -> Result<PackageId, Vec<Diagnostic>> {
        if let Some(&id) = self.names.get(name) {
            if !self.is_toolchain(id) {
                return Err(import_error(
                    range,
                    format!(
                        "package `{name}` resolves to multiple roots: {} ({}) and {} -> {name} (Rinzu {RELEASE})",
                        self.graph.dependency_path(id),
                        self.manifest(id).root.display(),
                        self.graph.dependency_path(owner),
                    ),
                ));
            }
            if !self.checked.contains_key(&id) {
                return Err(import_error(
                    range,
                    format!(
                        "package dependency cycle: {} -> {name}",
                        self.graph.dependency_path(owner)
                    ),
                ));
            }
            self.graph.connect(owner, name.into(), id);
            return Ok(id);
        }
        let (location, manifest) = self.load(name, extra, range, sources)?;
        let id = self.graph.add_package(owner, location, manifest);
        self.names.insert(name.into(), id);
        self.graph.connect(owner, name.into(), id);
        let (manifest, artifact, _) = check_loaded(self, id, name, Vec::new(), &[], sources, None)?;
        self.remember(id, manifest, artifact);
        Ok(id)
    }

    fn load(
        &mut self,
        name: &str,
        extra: Option<&OfficialSdk>,
        range: SourceRange,
        sources: &mut SourceSnapshot<'_>,
    ) -> Result<(PackageLocation, Manifest), Vec<Diagnostic>> {
        let error = |message| import_error(range, message);
        let (root, files, digest) = if let Some(sdk) = extra {
            if self.official_root.is_none() {
                self.official_root = Some(official_sdk::installed_root(sources).map_err(error)?);
            }
            (
                self.official_root.as_ref().unwrap().join(&sdk.root),
                sdk.files.clone(),
                sdk.inventory_sha256.clone(),
            )
        } else {
            if self.standard_root.is_none() {
                let root = standard_library::resource_root().map_err(error)?;
                sources
                    .read_config(&root.join("registry.json"))
                    .map_err(|e| error(e.to_string()))?;
                self.standard_root = Some(root);
            }
            let short_name = name.strip_prefix("rinzu.").unwrap();
            let prefix = format!("packages/{short_name}/");
            let files = standard_library::resources()
                .into_iter()
                .filter_map(|(path, bytes)| {
                    path.strip_prefix(&prefix)
                        .map(|path| (PathBuf::from(path), sha256(bytes)))
                })
                .collect::<BTreeMap<_, _>>();
            let digest = sha256(&serde_json::to_vec(&files).map_err(|e| error(e.to_string()))?);
            (
                self.standard_root
                    .as_ref()
                    .unwrap()
                    .join("packages")
                    .join(short_name),
                files,
                digest,
            )
        };
        sources.register_toolchain(root.clone(), files.clone());
        if files.keys().any(|path| path.starts_with("native")) {
            sources
                .canonical_path(&root.join("native"))
                .map_err(|e| error(e.to_string()))?;
            official_sdk::check_native_files(&root, Path::new("native"), &files).map_err(error)?;
            for relative in files.keys().filter(|path| path.starts_with("native")) {
                let path = sources
                    .canonical_path(&root.join(relative))
                    .map_err(|e| error(e.to_string()))?;
                sources
                    .read_bytes(&path)
                    .map_err(|e| error(e.to_string()))?;
            }
        }
        let manifest = read_package_manifest(&root.join("rinzu.toml"), sources)?;
        if manifest.name != name
            || manifest.kind != PackageKind::Sdk
            || !manifest.dependencies.is_empty()
        {
            return Err(error(format!(
                "invalid toolchain SDK manifest for `{name}`"
            )));
        }
        let location = PackageLocation {
            root,
            source: PackageSource::Toolchain {
                name: name.into(),
                release: RELEASE.into(),
                inventory_sha256: digest,
            },
        };
        Ok((location, manifest))
    }

    fn remember(&mut self, id: PackageId, manifest: Manifest, artifact: FrontendArtifact) {
        self.checked.insert(
            id,
            (
                ResolvedSdkPackage {
                    manifest,
                    native_imports: Vec::new(),
                    source: artifact.source_interface,
                },
                artifact.sdk_packages,
            ),
        );
        self.graph.complete(id);
    }
}

pub(super) fn import_names() -> Result<impl Iterator<Item = &'static str>, Vec<Diagnostic>> {
    let official = official_sdk::public_names().map_err(|error| {
        vec![package_diagnostic(
            error,
            "use a matching Rinzu toolchain bundle",
        )]
    })?;
    Ok(standard_library::module_names().chain(official))
}

fn import_error(range: SourceRange, message: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        DiagnosticCode::PackageIo,
        range,
        message,
        "use the matching Rinzu toolchain bundle and verify the SDK dependency",
    )]
}

fn archive_error(range: SourceRange, name: &str) -> Vec<Diagnostic> {
    vec![Diagnostic::error(
        DiagnosticCode::InvalidModulePath,
        range,
        format!(
            "published SDK cannot import toolchain package `{name}` with additional native inputs"
        ),
        "keep native SDK imports in the application or a local SDK",
    )]
}
