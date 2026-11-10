//! Portable package pins and the layout of verified source archives.
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ArchiveDependency, Dependency, DependencySource, Manifest, PackageId, PackageSource,
    ResolvedGraph, valid_archive_locator, valid_package_name,
};

pub const FILE: &str = "rinzu.modules.lock";
pub const RECEIPT: &str = "receipt.json";
pub const MAX_LOCK_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_PACKAGES: usize = 10_000;
const SCHEMA: &str = "rinzu.modules-lock";
const VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivePin {
    pub locator: String,
    pub sha256: String,
    pub tree_sha256: String,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub name: String,
    pub source: PackageSource,
    declarations_sha256: String,
    pub dependencies: BTreeMap<String, PackageId>,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulesLock {
    schema: String,
    version: u32,
    packages: Vec<LockedPackage>,
}

impl ModulesLock {
    pub fn from_graph(graph: &ResolvedGraph, root: &Manifest) -> Self {
        let packages = graph
            .packages()
            .map(|(id, node)| {
                let source = match &node.location.source {
                    PackageSource::Path(path) => PackageSource::Path(relative(&root.root, path)),
                    other => other.clone(),
                };
                let manifest = if id == ResolvedGraph::ROOT {
                    root
                } else {
                    &node.manifest
                };
                LockedPackage {
                    name: manifest.name.clone(),
                    source,
                    declarations_sha256: declarations(manifest),
                    dependencies: node.dependencies.clone(),
                }
            })
            .collect();
        Self {
            schema: SCHEMA.into(),
            version: VERSION,
            packages,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() as u64 > MAX_LOCK_BYTES {
            return Err("modules lock exceeds its size limit".into());
        }
        let lock: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid modules lock: {error}"))?;
        lock.validate()?;
        Ok(lock)
    }

    pub fn packages(&self) -> impl Iterator<Item = (PackageId, &LockedPackage)> {
        self.packages
            .iter()
            .enumerate()
            .map(|(index, package)| (PackageId(index), package))
    }

    pub(crate) fn same_resolution(&self, other: &Self) -> bool {
        self.packages.len() == other.packages.len()
            && self.packages.iter().zip(&other.packages).all(|(a, b)| {
                a.name == b.name && a.source == b.source && a.dependencies == b.dependencies
            })
    }

    pub fn target(&self, owner: PackageId, key: &str) -> Result<&LockedPackage, String> {
        self.packages
            .get(owner.0)
            .and_then(|node| node.dependencies.get(key))
            .and_then(|id| self.packages.get(id.0))
            .ok_or_else(|| format!("modules lock is missing dependency `{key}`"))
    }

    pub fn verify_manifest(&self, id: PackageId, manifest: &Manifest) -> Result<(), String> {
        if self.packages.get(id.0).is_some_and(|node| {
            node.name == manifest.name
                && node.declarations_sha256 == declarations(manifest)
                && node.dependencies.len() == manifest.dependencies.len()
                && manifest
                    .dependencies
                    .iter()
                    .all(|dependency| node.dependencies.contains_key(&dependency.name))
        }) {
            Ok(())
        } else {
            Err(format!(
                "modules lock does not match the dependencies of `{}`",
                manifest.name
            ))
        }
    }

    pub fn archive_pin(
        &self,
        project: &Path,
        id: PackageId,
        owner: &Manifest,
        dependency: &Dependency,
    ) -> Result<&ArchivePin, String> {
        let DependencySource::Archive(requirement) = &dependency.source else {
            return Err("expected an archive dependency".into());
        };
        let node = self.target(id, &dependency.name)?;
        if let PackageSource::Archive(pin) = &node.source
            && node.name == dependency.name
            && pin.locator == archive_locator(project, &owner.root, &requirement.archive)?
            && requirement
                .sha256
                .as_ref()
                .is_none_or(|digest| *digest == pin.sha256)
        {
            return Ok(pin);
        }
        Err(format!(
            "modules lock does not match archive dependency `{}` of `{}`",
            dependency.name, owner.name
        ))
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SCHEMA || self.version != VERSION {
            return Err("unsupported modules lock schema/version".into());
        }
        let count = self.packages.len();
        if count == 0
            || count > MAX_PACKAGES
            || self.packages[0].source != PackageSource::Path(PathBuf::from("."))
        {
            return Err("invalid modules lock root or package count".into());
        }
        let mut incoming = vec![0_usize; count];
        let mut names = HashSet::new();
        for (index, node) in self.packages.iter().enumerate() {
            if !valid_package_name(&node.name)
                || index != 0 && !names.insert(&node.name)
                || !valid_sha256(&node.declarations_sha256)
            {
                return Err("invalid or duplicate package in modules lock".into());
            }
            match &node.source {
                PackageSource::Path(path) if path.is_relative() && !path.as_os_str().is_empty() => {
                }
                PackageSource::Archive(pin)
                    if valid_archive_locator(&pin.locator)
                        && valid_sha256(&pin.sha256)
                        && valid_sha256(&pin.tree_sha256) => {}
                _ => return Err("invalid package source in modules lock".into()),
            }
            for (key, target) in &node.dependencies {
                let child = self
                    .packages
                    .get(target.0)
                    .ok_or("dangling modules lock dependency")?;
                if key != &child.name || target.0 == 0 {
                    return Err("invalid modules lock dependency".into());
                }
                if matches!(node.source, PackageSource::Archive(_))
                    && !matches!(&child.source, PackageSource::Archive(pin) if pin.locator.starts_with("https://"))
                {
                    return Err("published SDK dependencies must use pinned HTTPS archives".into());
                }
                incoming[target.0] += 1;
            }
        }
        let mut ready = vec![0];
        let mut visited = 0;
        while let Some(index) = ready.pop() {
            visited += 1;
            for child in self.packages[index].dependencies.values() {
                incoming[child.0] -= 1;
                if incoming[child.0] == 0 {
                    ready.push(child.0);
                }
            }
        }
        if visited != count {
            return Err("modules lock contains a cycle or unreachable packages".into());
        }
        Ok(())
    }
}

fn declarations(manifest: &Manifest) -> String {
    let bytes = serde_json::to_vec(&(
        &manifest.language,
        manifest.kind == crate::PackageKind::Sdk,
        &manifest.dependencies,
    ))
    .expect("manifest declarations are serializable");
    crate::standard_library::sha256(&bytes)
}

pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn archive_locator(project: &Path, owner: &Path, locator: &str) -> Result<String, String> {
    if locator.starts_with("https://") {
        return Ok(locator.into());
    }
    relative(project, &lexical_path(&owner.join(locator)))
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "archive path must be UTF-8".into())
}

pub fn validate_published_dependency(source: &DependencySource) -> Result<(), String> {
    if matches!(source, DependencySource::Archive(ArchiveDependency { archive, sha256: Some(_) }) if archive.starts_with("https://"))
    {
        Ok(())
    } else {
        Err("published SDK dependencies must use HTTPS archives with integrity pins".into())
    }
}

pub fn lexical_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

fn relative(root: &Path, path: &Path) -> PathBuf {
    let root: Vec<_> = root.components().collect();
    let path: Vec<_> = path.components().collect();
    let common = root.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let relative: PathBuf = std::iter::repeat_n(std::ffi::OsStr::new(".."), root.len() - common)
        .chain(path[common..].iter().map(|component| component.as_os_str()))
        .collect();
    if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative
    }
}

pub fn archive_entry(modules: &Path, digest: &str) -> PathBuf {
    modules.join("store/sha256").join(digest)
}

pub fn archive_package_root(modules: &Path, digest: &str) -> io::Result<PathBuf> {
    let package = archive_entry(modules, digest).join("package");
    for directory in package
        .ancestors()
        .take_while(|path| path.starts_with(modules))
    {
        if !std::fs::symlink_metadata(directory)?.is_dir() {
            return Err(io::Error::other(format!(
                "archive directory is not a real directory: {}",
                directory.display()
            )));
        }
    }
    Ok(package)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveReceipt {
    pub archive_sha256: String,
    pub files: BTreeMap<PathBuf, String>,
}

impl ArchiveReceipt {
    pub fn parse(bytes: &[u8], pin: &ArchivePin) -> Result<Self, String> {
        if crate::standard_library::sha256(bytes) != pin.tree_sha256 {
            return Err("archive file inventory does not match its lock".into());
        }
        let receipt: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if receipt.archive_sha256 != pin.sha256
            || !receipt.files.contains_key(Path::new("rinzu.toml"))
            || receipt.files.iter().any(|(path, digest)| {
                path.as_os_str().is_empty()
                    || path
                        .components()
                        .any(|c| !matches!(c, Component::Normal(_)))
                    || !valid_sha256(digest)
            })
        {
            return Err("invalid archive file inventory".into());
        }
        Ok(receipt)
    }
}
