use rinzu_bindgen::source_package::sha256;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod retention;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: String,
    key: String,
    files: BTreeMap<String, String>,
    facade: Option<String>,
}

pub fn directory(
    root: &Path,
    _application_root: &Path,
    wire: &[u8],
) -> Result<(PathBuf, String), String> {
    directory_in(root, &rinzu_compiler::standard_library::cache_root()?, wire)
}

pub(crate) fn runtime_directory(root: &Path, wire: &[u8]) -> Result<(PathBuf, String), String> {
    user_directory(root, "jit-runtime-v1", wire)
}

pub(crate) fn user_directory(
    root: &Path,
    namespace: &str,
    wire: &[u8],
) -> Result<(PathBuf, String), String> {
    directory_in_namespace(
        root,
        &rinzu_compiler::standard_library::cache_root()?,
        namespace,
        wire,
    )
}

fn directory_in(root: &Path, cache_root: &Path, wire: &[u8]) -> Result<(PathBuf, String), String> {
    directory_in_namespace(root, cache_root, "source-v4", wire)
}

fn directory_in_namespace(
    root: &Path,
    cache_root: &Path,
    namespace: &str,
    wire: &[u8],
) -> Result<(PathBuf, String), String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let cache_root = cache_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let environment =
        serde_json::to_vec(&(root, namespace, sha256(wire))).map_err(|error| error.to_string())?;
    let key = sha256(&environment);
    let mut directory = cache_root.clone();
    for component in [namespace, &key] {
        directory.push(component);
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("source cache directory cannot be a symlink".to_owned());
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err("source cache path is not a directory".to_owned());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::create_dir(&directory) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                    Err(e) => return Err(e.to_string()),
                }
            }
            Err(error) => return Err(error.to_string()),
        }
        if !directory
            .canonicalize()
            .map_err(|error| error.to_string())?
            .starts_with(&cache_root)
        {
            return Err("source cache path escaped the user cache".to_owned());
        }
    }
    Ok((directory, key))
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.file_type().is_file())?;
    std::fs::read(path).ok()
}

fn facade_name() -> String {
    format!("facade{}", std::env::consts::DLL_SUFFIX)
}

pub fn restore(
    directory: &Path,
    key: &str,
    expected: &BTreeMap<String, Vec<u8>>,
    facade: Option<&Path>,
) -> Result<bool, String> {
    let _lease = retention::lock(directory, false)?;
    let Some(receipt) = read_file(&directory.join("receipt.json"))
        .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok())
    else {
        return Ok(false);
    };
    let expected_digests: BTreeMap<_, _> = expected
        .iter()
        .map(|(name, bytes)| (name.clone(), sha256(bytes)))
        .collect();
    if receipt.schema != "rinzu.source-cache-v4"
        || receipt.key != key
        || receipt.files != expected_digests
        || receipt.facade.is_some() != facade.is_some()
    {
        return Ok(false);
    }
    for (name, bytes) in expected {
        if read_file(&directory.join(name)).as_ref() != Some(bytes) {
            return Ok(false);
        }
    }
    if let Some(destination) = facade {
        let bytes = read_file(&directory.join(facade_name()));
        // Preserve Linux entries written before the host suffix was used.
        #[cfg(target_os = "linux")]
        let bytes = bytes.or_else(|| read_file(&directory.join("facade.dylib")));
        let Some(bytes) = bytes else {
            return Ok(false);
        };
        if receipt.facade.as_deref() != Some(sha256(&bytes).as_str()) {
            return Ok(false);
        }
        std::fs::write(destination, bytes).map_err(|error| error.to_string())?;
    }
    Ok(true)
}

/// Readers copy one sealed generation under a shared namespace lease. Writers
/// publish the receipt last and reclaim old generations under the exclusive lease.
pub fn store(
    directory: &Path,
    key: &str,
    files: &BTreeMap<String, Vec<u8>>,
    facade: Option<&Path>,
) -> Result<(), String> {
    let _lease = retention::lock(directory, true)?;
    match std::fs::create_dir(directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error.to_string()),
    }
    if !directory
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return Err("source cache generation is not a regular directory".into());
    }
    fn replace(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let temporary = directory.join(format!(".pending-{}-{nonce}", std::process::id()));
        let mut file = std::fs::File::create_new(&temporary).map_err(|error| error.to_string())?;
        let result = file
            .write_all(bytes)
            .and_then(|()| std::fs::rename(&temporary, directory.join(name)));
        // Also remove the unpublished file if writing or renaming failed.
        let _ = std::fs::remove_file(&temporary);
        result.map_err(|error| error.to_string())
    }
    let facade = facade
        .map(|path| std::fs::read(path).map_err(|error| error.to_string()))
        .transpose()?;
    let receipt = Receipt {
        schema: "rinzu.source-cache-v4".to_owned(),
        key: key.to_owned(),
        files: files
            .iter()
            .map(|(name, bytes)| (name.clone(), sha256(bytes)))
            .collect(),
        facade: facade.as_ref().map(|bytes| sha256(bytes)),
    };
    let receipt = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
    let bytes = files.values().map(|bytes| bytes.len() as u64).sum::<u64>()
        + facade.as_ref().map_or(0, |bytes| bytes.len() as u64)
        + receipt.len() as u64;
    if bytes > retention::LIMITS.bytes {
        return Err("source cache generation exceeds the namespace capacity".into());
    }
    for (name, bytes) in files {
        replace(directory, name, bytes)?;
    }
    if let Some(bytes) = &facade {
        replace(directory, &facade_name(), bytes)?;
    }
    replace(directory, "receipt.json", &receipt)?;
    retention::collect(
        directory.parent().ok_or("missing cache namespace")?,
        directory,
        retention::LIMITS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_generation_and_symlink_cache_are_never_restored() {
        let root = std::env::temp_dir().join(format!("rinzu-cache-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let cache_root = root.join("user-cache");
        std::fs::create_dir(&cache_root).unwrap();
        let old_cache = cache_root.join("source-v3");
        std::fs::create_dir(&old_cache).unwrap();
        std::fs::write(old_cache.join("sentinel"), b"old cache retained").unwrap();
        let (directory, key) = directory_in(&root, &cache_root, b"wire").unwrap();
        assert!(!root.join(".rinzu").exists());
        let files = BTreeMap::from([("source-table.c".to_owned(), b"table".to_vec())]);
        assert!(!restore(&directory, &key, &files, None).unwrap());
        store(&directory, &key, &files, None).unwrap();
        assert!(restore(&directory, &key, &files, None).unwrap());
        let receipt = directory.join("receipt.json");
        for schema in ["rinzu.source-cache-v2", "rinzu.source-cache-v3"] {
            let old = std::fs::read_to_string(&receipt)
                .unwrap()
                .replace("rinzu.source-cache-v4", schema);
            std::fs::write(&receipt, old).unwrap();
            assert!(!restore(&directory, &key, &files, None).unwrap());
            store(&directory, &key, &files, None).unwrap();
        }
        assert_eq!(
            std::fs::read(old_cache.join("sentinel")).unwrap(),
            b"old cache retained"
        );
        store(&directory, &key, &files, None).unwrap();
        std::fs::write(directory.join("source-table.c"), b"different generation").unwrap();
        assert!(!restore(&directory, &key, &files, None).unwrap());
        std::fs::remove_file(directory.join("source-table.c")).unwrap();
        std::os::unix::fs::symlink(root.join("missing"), directory.join("source-table.c")).unwrap();
        assert!(!restore(&directory, &key, &files, None).unwrap());
        store(&directory, &key, &files, None).unwrap();
        assert!(restore(&directory, &key, &files, None).unwrap());
        std::fs::remove_dir_all(cache_root.join("source-v4")).unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), cache_root.join("source-v4")).unwrap();
        assert!(directory_in(&root, &cache_root, b"wire").is_err());
        std::fs::remove_file(cache_root.join("source-v4")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
/// Load a completed generation with the caller's exact output names.
pub fn completed(
    directory: &Path,
    key: &str,
    names: &BTreeSet<String>,
) -> Option<BTreeMap<String, Vec<u8>>> {
    completed_with_inventory(directory, key, &[], |_| Some((names.clone(), ())))
        .map(|(files, ())| files)
}

/// Check fixed metadata first when it describes a variable output inventory.
pub(crate) fn completed_with_inventory<T>(
    directory: &Path,
    key: &str,
    metadata: &[&str],
    inventory: impl FnOnce(&BTreeMap<String, Vec<u8>>) -> Option<(BTreeSet<String>, T)>,
) -> Option<(BTreeMap<String, Vec<u8>>, T)> {
    completed_with_reader(directory, key, metadata, inventory, read_file)
}

pub(crate) fn completed_with_reader<T>(
    directory: &Path,
    key: &str,
    metadata: &[&str],
    inventory: impl FnOnce(&BTreeMap<String, Vec<u8>>) -> Option<(BTreeSet<String>, T)>,
    read: impl Fn(&Path) -> Option<Vec<u8>>,
) -> Option<(BTreeMap<String, Vec<u8>>, T)> {
    let _lease = retention::lock(directory, false).ok()?;
    let receipt: Receipt = serde_json::from_slice(&read(&directory.join("receipt.json"))?).ok()?;
    if receipt.schema != "rinzu.source-cache-v4" || receipt.key != key || receipt.facade.is_some() {
        return None;
    }
    let read_member = |name: &str, digest: &str| {
        let path = Path::new(name);
        if path.components().count() != 1
            || !matches!(
                path.components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return None;
        }
        let bytes = read(&directory.join(path))?;
        (sha256(&bytes) == digest).then_some(bytes)
    };
    let mut files = BTreeMap::new();
    for &name in metadata {
        files.insert(
            name.to_owned(),
            read_member(name, receipt.files.get(name)?)?,
        );
    }
    let (names, data) = inventory(&files)?;
    if receipt.files.keys().ne(names.iter()) {
        return None;
    }
    for (name, digest) in receipt.files {
        if let std::collections::btree_map::Entry::Vacant(entry) = files.entry(name) {
            let bytes = read_member(entry.key(), &digest)?;
            entry.insert(bytes);
        }
    }
    Some((files, data))
}

#[cfg(test)]
mod completed_tests {
    use super::*;

    #[test]
    fn completed_generations_require_exact_names_regular_files_and_current_bytes() {
        let root = crate::verify_capture::TemporaryDirectory(
            std::env::temp_dir().join(format!("rinzu-completed-cache-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&root.0);
        std::fs::create_dir_all(&root.0).unwrap();
        let (directory, key) = directory_in(&root.0, &root.0, b"completed-unit").unwrap();
        let files = BTreeMap::from([("source-table.c".into(), b"original".to_vec())]);
        let names = files.keys().cloned().collect();
        store(&directory, &key, &files, None).unwrap();
        assert_eq!(completed(&directory, &key, &names), Some(files.clone()));
        assert!(completed(&directory, &key, &Default::default()).is_none());
        std::fs::write(directory.join("source-table.c"), b"changed").unwrap();
        assert!(completed(&directory, &key, &names).is_none());
        std::fs::remove_file(directory.join("source-table.c")).unwrap();
        let external = root.0.join("external");
        std::fs::write(&external, b"original").unwrap();
        std::os::unix::fs::symlink(external, directory.join("source-table.c")).unwrap();
        assert!(completed(&directory, &key, &names).is_none());
    }
}
