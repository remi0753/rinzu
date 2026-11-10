//! Explicit source acquisition. The compiler consumes the resulting graph offline.
mod archive;
mod project;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rinzu_compiler::modules_lock::{
    archive_locator, archive_package_root, lexical_path, validate_published_dependency,
};
use rinzu_compiler::{
    ArchiveDependency, ArchivePin, DependencySource, Diagnostic, DiagnosticCode, FileId, Manifest,
    ModulesLock, PackageId, PackageKind, PackageLocation, PackageProvider, PackageSource,
    ResolvedGraph, ResolvedPackage, SourceCache, SourceRange, SourceSnapshot, find_manifest,
    load_manifest, load_manifest_text, load_resolved_manifest, resolve_installed_graph,
    resolve_package_graph, set_archive_dependency, valid_archive_locator, valid_package_name,
};

use archive::{Request, Store};
use project::State;

const USAGE: &str = "usage: rinzu modules add <SDK name> <HTTPS URL or archive path>\n       rinzu modules sync [--offline]";
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub fn run(arguments: impl Iterator<Item = String>) -> std::result::Result<(), String> {
    execute(&arguments.collect::<Vec<_>>()).map_err(|error| error.to_string())
}

fn execute(arguments: &[String]) -> Result<()> {
    let command: Vec<_> = arguments.iter().map(String::as_str).collect();
    let offline = match command.as_slice() {
        [] | ["help" | "-h" | "--help"] | ["add" | "sync", "-h" | "--help"] => {
            println!("{USAGE}");
            return Ok(());
        }
        ["sync", "--offline"] => true,
        ["sync"] | ["add", _, _] => false,
        _ => return Err(USAGE.into()),
    };
    let cwd = std::env::current_dir()?.canonicalize()?;
    let path = find_manifest(&cwd).map_err(manifest_error)?;
    let project = path.parent().expect("manifest has parent");
    let state = project::snapshot(project)?;
    let root = load_manifest_text(&path, &state.manifest).map_err(manifest_error)?;
    let lock = state
        .lock
        .as_ref()
        .map(|text| ModulesLock::parse(text.as_bytes()))
        .transpose()?;
    if let Some(lock) = &lock {
        verify_local(project, &root, lock)?;
    }
    let store = Store { project, offline };
    if let ["add", name, source] = command.as_slice() {
        add(&store, &cwd, state, root, lock.as_ref(), name, source)
    } else {
        sync(
            &store,
            &state,
            root,
            lock.as_ref()
                .ok_or("missing rinzu.modules.lock; add an SDK first")?,
        )
    }
}

fn add(
    store: &Store<'_>,
    cwd: &Path,
    state: State,
    manifest: Manifest,
    lock: Option<&ModulesLock>,
    name: &str,
    source: &str,
) -> Result<()> {
    if !valid_package_name(name) || rinzu_compiler::standard_library::registered(name) {
        return Err(format!("invalid or reserved SDK name: {name}").into());
    }
    let locator = archive_locator(store.project, cwd, source)?;
    if source.is_empty()
        || source.contains("://") && !source.starts_with("https://")
        || !valid_archive_locator(&locator)
    {
        return Err("expected an HTTPS URL or local archive path".into());
    }
    let mut dependency = ArchiveDependency {
        archive: locator.clone(),
        sha256: None,
    };
    if let Some(existing) = manifest
        .dependencies
        .iter()
        .find(|dependency| dependency.name == name)
    {
        match &existing.source {
            DependencySource::Archive(archive)
                if archive_locator(store.project, &manifest.root, &archive.archive)? == locator =>
            {
                if let Some(lock) = lock {
                    return sync(store, &state, manifest, lock);
                }
                dependency = archive.clone();
            }
            _ => return Err(format!("dependency `{name}` already has a different source").into()),
        }
    }

    let path = store.project.join("rinzu.toml");
    let mut text = set_archive_dependency(&path, &state.manifest, name, &dependency)
        .map_err(manifest_error)?;
    let root = load_manifest_text(&path, &text).map_err(manifest_error)?;
    let pins = lock
        .into_iter()
        .flat_map(ModulesLock::packages)
        .filter_map(|(_, package)| {
            if let PackageSource::Archive(pin) = &package.source {
                Some((pin.locator.clone(), pin.clone()))
            } else {
                None
            }
        })
        .collect();
    let mut cache = SourceCache::default();
    let mut provider = AcquiringPackages {
        store,
        pins,
        installed: HashMap::new(),
        sources: SourceSnapshot::new(&mut cache, Default::default()),
    };
    let graph = resolve_package_graph(root.clone(), &mut provider).map_err(diagnostics)?;
    let id = graph.package(ResolvedGraph::ROOT).dependencies[name];
    let PackageSource::Archive(pin) = &graph.package(id).location.source else {
        return Err("new SDK did not resolve to an archive".into());
    };
    // When the root is itself a library, its direct archive declaration can
    // be published with the integrity that its consumers must enforce.
    if root.kind == PackageKind::Sdk {
        dependency.sha256 = Some(pin.sha256.clone());
        text = set_archive_dependency(&path, &state.manifest, name, &dependency)
            .map_err(manifest_error)?;
    }
    let root = load_manifest_text(&path, &text).map_err(manifest_error)?;
    let mut lock = serde_json::to_string_pretty(&ModulesLock::from_graph(&graph, &root))?;
    lock.push('\n');
    let lock_value = ModulesLock::parse(lock.as_bytes())?;
    verify_local(store.project, &root, &lock_value)?;
    project::update(
        store.project,
        &state,
        State {
            manifest: text,
            lock: Some(lock),
        },
    )?;
    println!("added {name} (sha256:{})", pin.sha256);
    Ok(())
}

fn sync(store: &Store<'_>, state: &State, root: Manifest, lock: &ModulesLock) -> Result<()> {
    let mut unique = BTreeMap::<_, Request>::new();
    for (_, package) in lock.packages() {
        if let PackageSource::Archive(pin) = &package.source {
            match unique.entry(&pin.sha256) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Request::pinned(&package.name, pin));
                }
                std::collections::btree_map::Entry::Occupied(entry) => {
                    if entry.get().tree_sha256.as_ref() != Some(&pin.tree_sha256) {
                        return Err(
                            "modules lock pins different inventories for the same archive".into(),
                        );
                    }
                }
            }
        }
    }
    let requests: Vec<_> = unique.into_values().collect();
    store.materialize(&requests)?;
    let _guard = project::guard(store.project)?;
    if &State::read(store.project)? != state {
        return Err(
            "the manifest or modules lock changed during acquisition; retry the command".into(),
        );
    }
    let mut cache = SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    resolve_installed_graph(root, &mut sources).map_err(diagnostics)?;
    println!("synced {} SDK archive(s)", requests.len());
    Ok(())
}

fn verify_local(project: &Path, root: &Manifest, lock: &ModulesLock) -> Result<()> {
    for (id, node) in lock.packages() {
        let PackageSource::Path(path) = &node.source else {
            continue;
        };
        let manifest = if id == ResolvedGraph::ROOT {
            root.clone()
        } else {
            load_manifest(&project.join(path).join("rinzu.toml")).map_err(manifest_error)?
        };
        lock.verify_manifest(id, &manifest)?;
        for dependency in &manifest.dependencies {
            match &dependency.source {
                DependencySource::Path(path) => {
                    let target = lock.target(id, &dependency.name)?;
                    if !matches!(&target.source, PackageSource::Path(expected)
                        if manifest.root.join(path).canonicalize().ok() == Some(lexical_path(&project.join(expected))))
                    {
                        return Err(format!(
                            "modules lock does not match path dependency `{}`",
                            dependency.name
                        )
                        .into());
                    }
                }
                DependencySource::Archive(_) => {
                    lock.archive_pin(project, id, &manifest, dependency)?;
                }
            }
        }
    }
    Ok(())
}

struct AcquiringPackages<'a, 'cache> {
    store: &'a Store<'a>,
    pins: HashMap<String, ArchivePin>,
    installed: HashMap<String, ArchivePin>,
    sources: SourceSnapshot<'cache>,
}

enum Input {
    Local(PathBuf),
    Archive(String),
}

impl AcquiringPackages<'_, '_> {
    fn locate(&mut self, owner: &ResolvedPackage) -> Result<Vec<PackageLocation>> {
        let mut inputs = Vec::new();
        let mut requests = BTreeMap::new();
        for dependency in &owner.manifest.dependencies {
            if matches!(owner.location.source, PackageSource::Archive(_)) {
                validate_published_dependency(&dependency.source)?;
            }
            match &dependency.source {
                DependencySource::Path(path) => {
                    inputs.push(Input::Local(owner.location.root.join(path).canonicalize()?));
                }
                DependencySource::Archive(archive) => {
                    let locator = archive_locator(
                        self.store.project,
                        &owner.location.root,
                        &archive.archive,
                    )?;
                    if let Some(pinned) = self
                        .installed
                        .get(&locator)
                        .or_else(|| self.pins.get(&locator))
                        && archive
                            .sha256
                            .as_ref()
                            .is_some_and(|expected| *expected != pinned.sha256)
                    {
                        return Err(format!(
                            "conflicting SHA-256 pins for `{}` required by `{}`",
                            dependency.name, owner.manifest.name
                        )
                        .into());
                    }
                    if !self.installed.contains_key(&locator) {
                        let request = self.pins.get(&locator).map_or_else(
                            || Request {
                                name: dependency.name.clone(),
                                locator: locator.clone(),
                                sha256: archive.sha256.clone(),
                                tree_sha256: None,
                            },
                            |pin| Request::pinned(&dependency.name, pin),
                        );
                        if let Some(previous) = requests.insert(locator.clone(), request.clone())
                            && previous.sha256 != request.sha256
                        {
                            return Err(format!("conflicting SHA-256 pins for {locator}").into());
                        }
                    }
                    inputs.push(Input::Archive(locator));
                }
            }
        }
        let requests: Vec<_> = requests.into_values().collect();
        for pin in self.store.materialize(&requests)? {
            self.installed.insert(pin.locator.clone(), pin);
        }
        let modules = self.store.project.join("modules");
        inputs
            .into_iter()
            .map(|input| match input {
                Input::Local(root) => Ok(PackageLocation::local(root)),
                Input::Archive(locator) => {
                    let pin = self.installed[&locator].clone();
                    let root = archive_package_root(&modules, &pin.sha256)?;
                    Ok(PackageLocation {
                        root,
                        source: PackageSource::Archive(pin),
                    })
                }
            })
            .collect()
    }
}

impl PackageProvider for AcquiringPackages<'_, '_> {
    fn dependencies(
        &mut self,
        _id: PackageId,
        owner: &ResolvedPackage,
    ) -> std::result::Result<Vec<PackageLocation>, Vec<Diagnostic>> {
        self.locate(owner).map_err(|error| {
            vec![
                Diagnostic::error(
                    DiagnosticCode::PackageIo,
                    SourceRange::empty(FileId::MANIFEST, 0),
                    error.to_string(),
                    "check the dependency declaration and its archive",
                )
                .with_path(owner.location.root.join("rinzu.toml")),
            ]
        })
    }

    fn load(
        &mut self,
        location: &PackageLocation,
    ) -> std::result::Result<Manifest, Vec<Diagnostic>> {
        load_resolved_manifest(location, &mut self.sources)
    }
}

fn manifest_error(error: rinzu_compiler::ManifestError) -> String {
    error.into_diagnostic().message
}

fn diagnostics(errors: Vec<Diagnostic>) -> String {
    errors
        .into_iter()
        .map(|error| error.message)
        .collect::<Vec<_>>()
        .join("\n")
}
