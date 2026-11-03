use rinzu_bindgen::source_package::sha256;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

fn directory_in(root: &Path, cache_root: &Path, wire: &[u8]) -> Result<(PathBuf, String), String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let cache_root = cache_root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let environment = serde_json::to_vec(&(root.clone(), "source-v4", sha256(wire)))
        .map_err(|error| error.to_string())?;
    let key = sha256(&environment);
    let mut directory = cache_root.clone();
    for component in ["source-v4", &key] {
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

pub fn restore(
    directory: &Path,
    key: &str,
    expected: &BTreeMap<String, Vec<u8>>,
    facade: Option<&Path>,
) -> Result<bool, String> {
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
        let Some(bytes) = read_file(&directory.join("facade.dylib")) else {
            return Ok(false);
        };
        if receipt.facade.as_deref() != Some(sha256(&bytes).as_str()) {
            return Ok(false);
        }
        std::fs::write(destination, bytes).map_err(|error| error.to_string())?;
    }
    Ok(true)
}

/// Each file is replaced atomically; the receipt is published last. Mixed generations never restore.
pub fn store(
    directory: &Path,
    key: &str,
    files: &BTreeMap<String, Vec<u8>>,
    facade: Option<&Path>,
) -> Result<(), String> {
    fn replace(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let temporary = directory.join(format!(".pending-{}-{nonce}", std::process::id()));
        std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, directory.join(name)).map_err(|error| error.to_string())?;
        Ok(())
    }
    for (name, bytes) in files {
        replace(directory, name, bytes)?;
    }
    let facade = facade
        .map(|path| std::fs::read(path).map_err(|error| error.to_string()))
        .transpose()?;
    if let Some(bytes) = &facade {
        replace(directory, "facade.dylib", bytes)?;
    }
    let receipt = Receipt {
        schema: "rinzu.source-cache-v4".to_owned(),
        key: key.to_owned(),
        files: files
            .iter()
            .map(|(name, bytes)| (name.clone(), sha256(bytes)))
            .collect(),
        facade: facade.as_ref().map(|bytes| sha256(bytes)),
    };
    replace(
        directory,
        "receipt.json",
        &serde_json::to_vec(&receipt).map_err(|error| error.to_string())?,
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
/// Load a completed SDK generation before regenerating its final descriptor and
/// linking its facade. The caller supplies the freshly validated input key and
/// the exact allowed output names. Every output is verified before any is used.
pub fn completed(
    directory: &Path,
    key: &str,
    names: &std::collections::BTreeSet<String>,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let receipt: Receipt =
        serde_json::from_slice(&read_file(&directory.join("receipt.json"))?).ok()?;
    if receipt.schema != "rinzu.source-cache-v4"
        || receipt.key != key
        || receipt.facade.is_some()
        || receipt.files.keys().ne(names.iter())
    {
        return None;
    }
    let mut files = BTreeMap::new();
    for (name, digest) in receipt.files {
        let path = Path::new(&name);
        if path.components().count() != 1
            || !matches!(
                path.components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return None;
        }
        let bytes = read_file(&directory.join(&name))?;
        if sha256(&bytes) != digest {
            return None;
        }
        files.insert(name, bytes);
    }
    Some(files)
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
