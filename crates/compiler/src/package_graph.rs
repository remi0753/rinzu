//! Dependency discovery independent of source checking and acquisition.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::package::package_diagnostic;
use crate::{ArchivePin, Diagnostic, Manifest, PackageKind};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackageId(pub(crate) usize);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageSource {
    Path(PathBuf),
    Archive(ArchivePin),
    Toolchain {
        name: String,
        release: String,
        inventory_sha256: String,
    },
}

#[derive(Clone, Debug)]
pub struct PackageLocation {
    pub root: PathBuf,
    pub source: PackageSource,
}

impl PackageLocation {
    pub fn local(root: PathBuf) -> Self {
        Self {
            source: PackageSource::Path(root.clone()),
            root,
        }
    }
}

#[derive(Debug)]
pub struct ResolvedPackage {
    pub location: PackageLocation,
    pub manifest: Manifest,
    pub dependencies: BTreeMap<String, PackageId>,
}

#[derive(Debug)]
pub struct ResolvedGraph {
    packages: Vec<ResolvedPackage>,
    order: Vec<PackageId>,
    parents: Vec<Option<PackageId>>,
}

impl ResolvedGraph {
    pub const ROOT: PackageId = PackageId(0);

    pub(crate) fn new(root: Manifest) -> Self {
        Self {
            packages: vec![ResolvedPackage {
                location: PackageLocation::local(root.root.clone()),
                manifest: root,
                dependencies: BTreeMap::new(),
            }],
            order: Vec::new(),
            parents: vec![None],
        }
    }

    pub fn package(&self, id: PackageId) -> &ResolvedPackage {
        &self.packages[id.0]
    }

    pub fn packages(&self) -> impl Iterator<Item = (PackageId, &ResolvedPackage)> {
        self.packages
            .iter()
            .enumerate()
            .map(|(index, package)| (PackageId(index), package))
    }

    pub fn dependency_order(&self) -> &[PackageId] {
        &self.order
    }

    pub(crate) fn dependency_path(&self, id: PackageId) -> String {
        let mut names = Vec::new();
        let mut current = Some(id);
        while let Some(id) = current {
            names.push(self.package(id).manifest.name.as_str());
            current = self.parents[id.0];
        }
        names.reverse();
        names.join(" -> ")
    }

    pub(crate) fn add_package(
        &mut self,
        parent: PackageId,
        location: PackageLocation,
        manifest: Manifest,
    ) -> PackageId {
        let id = PackageId(self.packages.len());
        self.packages.push(ResolvedPackage {
            location,
            manifest,
            dependencies: BTreeMap::new(),
        });
        self.parents.push(Some(parent));
        id
    }

    pub(crate) fn connect(&mut self, parent: PackageId, name: String, dependency: PackageId) {
        self.packages[parent.0]
            .dependencies
            .insert(name, dependency);
    }

    pub(crate) fn take_order(&mut self) -> Vec<PackageId> {
        std::mem::take(&mut self.order)
    }

    pub(crate) fn complete(&mut self, id: PackageId) {
        self.order.push(id);
    }
}

/// A provider selects the concrete inputs for the declarations of each owner.
/// Discovery can use locally checked paths, a lock, or metadata from a source.
pub trait PackageProvider {
    fn dependencies(
        &mut self,
        id: PackageId,
        owner: &ResolvedPackage,
    ) -> Result<Vec<PackageLocation>, Vec<Diagnostic>>;

    fn load(&mut self, location: &PackageLocation) -> Result<Manifest, Vec<Diagnostic>>;
}

pub fn resolve_package_graph(
    root: Manifest,
    provider: &mut impl PackageProvider,
) -> Result<ResolvedGraph, Vec<Diagnostic>> {
    let mut timing = crate::timing::span("frontend.package-graph").detail(&root.name);
    let mut graph = ResolvedGraph::new(root);
    let root = ResolvedGraph::ROOT;
    let node = graph.package(root);
    let mut identities = HashMap::from([(node.location.source.clone(), root)]);
    let mut names = HashMap::new();
    if node.manifest.kind == PackageKind::Sdk {
        names.insert(node.manifest.name.clone(), root);
    }
    let mut pending = vec![(root, targets(root, graph.package(root), provider)?)];
    let mut complete = HashSet::new();
    while let Some((parent, dependencies)) = pending.last_mut() {
        let parent = *parent;
        let Some((index, location)) = dependencies.next() else {
            graph.complete(parent);
            complete.insert(parent);
            pending.pop();
            continue;
        };
        let name = graph.package(parent).manifest.dependencies[index]
            .name
            .clone();
        let path = location.root.join("rinzu.toml");
        let mut discovered = false;
        let id = if let Some(&id) = identities.get(&location.source) {
            if !complete.contains(&id) {
                let mut route = graph.dependency_path(parent);
                route.push_str(&format!(" -> {name}"));
                return Err(vec![
                    package_diagnostic(
                        format!("package dependency cycle: {route}"),
                        "remove a dependency so the package graph is acyclic",
                    )
                    .with_path(&path),
                ]);
            }
            check_name(&name, &graph.package(id).manifest)?;
            id
        } else {
            if graph.packages.len() == crate::modules_lock::MAX_PACKAGES {
                return Err(vec![
                    package_diagnostic(
                        "package graph exceeds its node limit",
                        "reduce the number of dependencies",
                    )
                    .with_path(&path),
                ]);
            }
            let manifest = provider.load(&location)?;
            check_name(&name, &manifest)?;
            if let Some(&previous) = names.get(&name) {
                return Err(vec![
                    package_diagnostic(
                        format!(
                            "package `{name}` resolves to multiple roots: {} ({}) and {} -> {name} ({})",
                            graph.dependency_path(previous),
                            graph.package(previous).location.root.display(),
                            graph.dependency_path(parent),
                            location.root.display()
                        ),
                        "use one canonical package identity",
                    )
                    .with_path(&path),
                ]);
            }
            let source = location.source.clone();
            let id = graph.add_package(parent, location, manifest);
            identities.insert(source, id);
            names.insert(name.clone(), id);
            discovered = true;
            id
        };
        graph.connect(parent, name, id);
        if discovered {
            pending.push((id, targets(id, graph.package(id), provider)?));
        }
    }
    timing.set_counter("packages", graph.packages.len() as u64);
    Ok(graph)
}

fn targets(
    id: PackageId,
    owner: &ResolvedPackage,
    provider: &mut impl PackageProvider,
) -> Result<std::iter::Enumerate<std::vec::IntoIter<PackageLocation>>, Vec<Diagnostic>> {
    if owner.manifest.dependencies.len() >= crate::modules_lock::MAX_PACKAGES {
        return Err(vec![
            package_diagnostic(
                "package has too many direct dependencies",
                "reduce the number of dependencies",
            )
            .with_path(owner.location.root.join("rinzu.toml")),
        ]);
    }
    let targets = provider.dependencies(id, owner)?;
    if targets.len() != owner.manifest.dependencies.len() {
        return Err(vec![package_diagnostic(
            "package provider returned an incomplete dependency list",
            "provide one resolved input for each dependency",
        )]);
    }
    Ok(targets.into_iter().enumerate())
}

fn check_name(name: &str, manifest: &Manifest) -> Result<(), Vec<Diagnostic>> {
    let (message, suggestion) = if manifest.name != name {
        (
            format!(
                "dependency key `{name}` resolves to package `{}`",
                manifest.name
            ),
            "make the dependency key equal the dependency package name",
        )
    } else if manifest.kind != PackageKind::Sdk || manifest.language == "0.1" {
        (
            format!("dependency `{name}` is not a supported source SDK package"),
            "set `language = \"0.5\"` and `kind = \"sdk\"` in the dependency",
        )
    } else {
        return Ok(());
    };
    Err(vec![
        package_diagnostic(message, suggestion).with_path(manifest.root.join("rinzu.toml")),
    ])
}
