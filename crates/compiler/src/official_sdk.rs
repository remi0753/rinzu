//! Additional SDKs shipped with a release, distinct from downloadable archives.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;

use crate::compatibility::{LANGUAGES, RELEASE};
use crate::standard_library::{self, TARGET, sha256};
use crate::{PackageKind, SourceSnapshot, load_manifest_text};

pub const BUNDLE_SCHEMA: &str = "rinzu.toolchain-bundle-v2";

struct Payload {
    name: &'static str,
    archive_compatible: bool,
    files: &'static [(&'static str, &'static [u8])],
}

macro_rules! sdk {
    ($directory:literal, $archive_compatible:literal, $($file:literal),+ $(,)?) => {
        Payload {
            name: concat!("rinzu.", $directory),
            archive_compatible: $archive_compatible,
            files: &[
                ("rinzu.toml", include_bytes!(concat!("../../../sdk/", $directory, "/rinzu.toml"))),
                ("README.md", include_bytes!(concat!("../../../sdk/", $directory, "/README.md"))),
                $(($file, include_bytes!(concat!("../../../sdk/", $directory, "/", $file))),)+
            ],
        }
    };
}

const PAYLOADS: &[Payload] = &[
    sdk!("codec", true, "src/codec.rinzu"),
    sdk!("log", true, "src/log.rinzu"),
    sdk!(
        "macos",
        false,
        "src/macos.rinzu",
        "src/document.rinzu",
        "src/document_files.rinzu",
        "src/image_files.rinzu",
        "native/src/macos.m",
        "native/include/rinzu_macos.h",
    ),
    sdk!(
        "sqlite",
        false,
        "src/sqlite.rinzu",
        "native/src/sqlite_source.c",
        "native/include/rinzu_sqlite.h",
    ),
    sdk!(
        "sdl2",
        false,
        "src/sdl2.rinzu",
        "native/src/sdl2_source.c",
        "native/include/rinzu_sdl2_source.h",
    ),
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfficialSdk {
    pub name: &'static str,
    pub root: PathBuf,
    pub language: String,
    pub archive_compatible: bool,
    pub files: BTreeMap<PathBuf, String>,
    pub inventory_sha256: String,
    pub public_modules: Vec<String>,
    pub native_libraries: Vec<String>,
    pub frameworks: Vec<String>,
    pub config_tools: Vec<String>,
}

struct Catalog {
    packages: BTreeMap<&'static str, OfficialSdk>,
    modules: BTreeMap<String, &'static str>,
    json: Value,
    digest: String,
}

pub fn registered(name: &str) -> bool {
    PAYLOADS.iter().any(|payload| payload.name == name)
}

pub(crate) fn archive_compatible(name: &str) -> bool {
    PAYLOADS
        .iter()
        .find(|payload| payload.name == name)
        .is_none_or(|payload| payload.archive_compatible)
}

fn catalog() -> Result<&'static Catalog, String> {
    static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();
    CATALOG
        .get_or_init(build_catalog)
        .as_ref()
        .map_err(Clone::clone)
}

fn build_catalog() -> Result<Catalog, String> {
    let mut packages = BTreeMap::new();
    let mut modules = BTreeMap::new();
    for payload in PAYLOADS {
        let root = Path::new("sdk").join(payload.name);
        let path = root.join("rinzu.toml");
        let manifest = load_manifest_text(
            &path,
            std::str::from_utf8(payload.files[0].1).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.into_diagnostic().message)?;
        if manifest.kind != PackageKind::Sdk
            || manifest.name != payload.name
            || !manifest.dependencies.is_empty()
            || !LANGUAGES.contains(&manifest.language.as_str())
            || payload.archive_compatible && manifest.native != Default::default()
        {
            return Err(format!("invalid bundled SDK manifest {}", path.display()));
        }
        let entry = crate::package::module_name_from_entry(&manifest.entry)
            .map_err(|errors| errors[0].message.clone())?;
        let mut public_modules = vec![manifest.name.clone()];
        public_modules.extend(
            manifest
                .exports
                .modules
                .iter()
                .filter(|module| **module != entry)
                .map(|module| format!("{}.{module}", manifest.name)),
        );
        for module in &public_modules {
            if standard_library::registered(module)
                || modules.insert(module.clone(), payload.name).is_some()
            {
                return Err(format!("duplicate bundled public module `{module}`"));
            }
        }
        let files: BTreeMap<PathBuf, String> = payload
            .files
            .iter()
            .map(|(relative, bytes)| (PathBuf::from(relative), sha256(bytes)))
            .collect();
        let inventory_sha256 =
            sha256(&serde_json::to_vec(&files).map_err(|error| error.to_string())?);
        packages.insert(
            payload.name,
            OfficialSdk {
                name: payload.name,
                root,
                language: manifest.language,
                archive_compatible: payload.archive_compatible,
                files,
                inventory_sha256,
                public_modules,
                native_libraries: manifest.native.libraries,
                frameworks: manifest.native.frameworks,
                config_tools: manifest.native.config_tools,
            },
        );
    }
    let json = serde_json::json!({
        "schema": "rinzu.official-sdks-v1",
        "toolchainVersion": RELEASE,
        "target": TARGET,
        "supportedLanguages": LANGUAGES,
        "packages": packages,
    });
    let digest = sha256(&serde_json::to_vec(&json).map_err(|error| error.to_string())?);
    Ok(Catalog {
        packages,
        modules,
        json,
        digest,
    })
}

pub(crate) fn public_names() -> Result<impl Iterator<Item = &'static str>, String> {
    Ok(catalog()?.modules.keys().map(String::as_str))
}

pub(crate) fn for_module(name: &str) -> Result<Option<&'static OfficialSdk>, String> {
    let catalog = catalog()?;
    Ok(catalog
        .modules
        .get(name)
        .map(|package| &catalog.packages[package]))
}

pub fn sdk(name: &str) -> Result<&'static OfficialSdk, String> {
    catalog()?
        .packages
        .get(name)
        .ok_or_else(|| format!("unknown additional official SDK `{name}`"))
}

pub fn catalog_metadata() -> Result<(&'static Value, &'static str), String> {
    let catalog = catalog()?;
    Ok((&catalog.json, &catalog.digest))
}

/// The bundle generator writes only explicitly listed release inputs.
pub fn write_resources(resources: &Path) -> Result<(), String> {
    catalog()?;
    for payload in PAYLOADS {
        let root = resources.join("sdk").join(payload.name);
        for (relative, bytes) in payload.files {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|error| error.to_string())?;
            std::fs::write(path, bytes).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

pub(crate) fn installed_root(sources: &mut SourceSnapshot<'_>) -> Result<PathBuf, String> {
    let root = standard_library::installed_resource_root()?
        .ok_or("official SDKs require a matching installed toolchain bundle")?;
    verify_catalog(&root, sources)?;
    Ok(root)
}

pub fn inspect_installation() -> Result<Option<PathBuf>, String> {
    let Some(root) = standard_library::installed_resource_root()? else {
        return Ok(None);
    };
    let mut cache = crate::SourceCache::default();
    let mut sources = SourceSnapshot::new(&mut cache, Default::default());
    verify_catalog(&root, &mut sources)?;
    standard_library::verify(&root)?;
    Ok(Some(root))
}

fn verify_catalog(root: &Path, sources: &mut SourceSnapshot<'_>) -> Result<(), String> {
    let parent = root
        .parent()
        .and_then(Path::parent)
        .filter(|_| root.ends_with("share/rinzu"))
        .ok_or("official SDK resources must be under the bundle's share/rinzu directory")?;
    let path = parent.join("bundle.json");
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot read toolchain bundle {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("invalid toolchain bundle metadata".into());
    }
    let bytes = sources
        .read_config(&path)
        .map_err(|error| error.to_string())?;
    let actual: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    let (expected, digest) = catalog_metadata()?;
    if actual["schema"] != BUNDLE_SCHEMA
        || actual["toolchainVersion"] != RELEASE
        || actual["target"] != TARGET
        || actual["resources"] != "share/rinzu"
        || actual["registrySha256"] != sha256(&standard_library::registry_bytes())
        || actual["officialSdkCatalogSha256"] != digest
        || actual.get("officialSdkCatalog") != Some(expected)
    {
        return Err("installed SDK catalog does not match this Rinzu toolchain".into());
    }
    Ok(())
}

/// Check native includes against the same inventory as the source files.
pub(crate) fn check_native_files(
    root: &Path,
    directory: &Path,
    files: &BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(root.join(directory)).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        let path = directory.join(entry.file_name());
        if kind.is_dir() {
            if !files.keys().any(|relative| relative.starts_with(&path)) {
                return Err(format!(
                    "unexpected bundled native directory {}",
                    root.join(path).display()
                ));
            }
            check_native_files(root, &path, files)?;
        } else if !kind.is_file() || !files.contains_key(&path) {
            return Err(format!(
                "unexpected bundled native input {}",
                root.join(path).display()
            ));
        }
    }
    Ok(())
}
