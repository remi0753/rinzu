use super::config::{Config, relative};
use rinzu_bindgen::source_package::sha256;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: usize = 256 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct Snapshot {
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    pub directories: BTreeSet<PathBuf>,
    pub icon: Option<Vec<u8>>,
    inputs: BTreeMap<PathBuf, (PathBuf, String, (u64, u64))>,
    total: usize,
}

fn file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        (metadata.len(), 0)
    }
}

pub fn confined(root: &Path, path: &Path) -> Result<PathBuf, String> {
    relative(path)?;
    let mut current = root.to_path_buf();
    for part in path.components() {
        current.push(part.as_os_str());
        if fs::symlink_metadata(&current)
            .map_err(|e| format!("read bundle input {}: {e}", current.display()))?
            .is_symlink()
        {
            return Err(format!(
                "bundle inputs cannot contain symlinks: {}",
                current.display()
            ));
        }
    }
    let canonical = current.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(root) {
        return Err("bundle input escaped its package root".into());
    }
    Ok(canonical)
}

pub fn read(root: &Path, path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let path = confined(root, path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Check the opened object before reading, without blocking on a substituted FIFO.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(&path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("bundle input must be a regular file".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err(format!(
            "bundle input exceeds its size limit: {}",
            path.display()
        ));
    }
    Ok(bytes)
}

impl Snapshot {
    pub fn capture(
        root: &Path,
        config_path: &Path,
        config_bytes: &[u8],
        config: &Config,
    ) -> Result<Self, String> {
        let mut result = Self::default();
        if result.capture_file(root, config_path, 65536)? != config_bytes {
            return Err("app configuration changed while reading".into());
        }
        for path in &config.resources {
            result.walk(root, path, 0)?;
        }
        if let Some(path) = &config.icon {
            result.icon = Some(result.capture_file(root, path, MAX_FILE)?);
        }
        Ok(result)
    }

    fn capture_file(&mut self, root: &Path, path: &Path, limit: u64) -> Result<Vec<u8>, String> {
        let full = confined(root, path)?;
        let before = fs::metadata(&full).map_err(|e| e.to_string())?;
        let bytes = read(root, path, limit)?;
        let after = fs::metadata(confined(root, path)?).map_err(|e| e.to_string())?;
        if file_identity(&before) != file_identity(&after) {
            return Err("bundle input identity changed while reading".into());
        }
        let identity = (full, sha256(&bytes), file_identity(&after));
        if self.inputs.get(path).is_some_and(|old| old != &identity) {
            return Err("bundle input changed while taking snapshot".into());
        }
        if self.inputs.insert(path.to_path_buf(), identity).is_none() {
            self.total += bytes.len();
            if self.total > MAX_TOTAL || self.inputs.len() > 10000 {
                return Err("bundle resources exceed 256 MiB / 10000 file limit".into());
            }
        }
        Ok(bytes)
    }

    fn walk(&mut self, root: &Path, path: &Path, depth: usize) -> Result<(), String> {
        if depth > 32 {
            return Err("bundle resource tree exceeds depth 32".into());
        }
        let full = confined(root, path)?;
        if full.is_file() {
            if self.files.contains_key(path) {
                return Err(format!("overlapping resource roots: {}", path.display()));
            }
            let bytes = self.capture_file(root, path, MAX_FILE)?;
            self.files.insert(path.to_path_buf(), bytes);
        } else if full.is_dir() {
            if !self.directories.insert(path.to_path_buf()) {
                return Err("overlapping resource directories".into());
            }
            if self.directories.len() > 10000 {
                return Err("too many resource directories".into());
            }
            let mut children = fs::read_dir(&full)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| e.file_name()).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            children.sort();
            for name in children {
                self.walk(root, &path.join(name), depth + 1)?;
            }
        } else {
            return Err("bundle resources must be regular files or directories".into());
        }
        Ok(())
    }

    pub fn verify(
        &self,
        root: &Path,
        config_path: &Path,
        config_bytes: &[u8],
        config: &Config,
    ) -> Result<(), String> {
        let current = Self::capture(root, config_path, config_bytes, config)?;
        if self.inputs != current.inputs || self.directories != current.directories {
            return Err("bundle inputs changed before publication; build again".into());
        }
        Ok(())
    }

    pub fn hashes(&self) -> BTreeMap<String, String> {
        self.inputs
            .iter()
            .map(|(p, (_, digest, _))| (p.to_string_lossy().into_owned(), digest.clone()))
            .collect()
    }
}

pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    fs::create_dir_all(path.parent().ok_or("output has no parent")?).map_err(|e| e.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    file.write_all(bytes).map_err(|e| e.to_string())
}
