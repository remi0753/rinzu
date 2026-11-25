//! Namespace leases cover disk reads/publication only. Once copied into memory
//! or a run directory, an artifact no longer pins its persistent generation.
use super::{Receipt, read_file};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub(super) generations: usize,
    pub(super) bytes: u64,
}

pub(super) const LIMITS: Limits = Limits {
    generations: 64,
    bytes: 256 * 1024 * 1024,
};

pub(super) fn lock(directory: &Path, exclusive: bool) -> Result<File, String> {
    let file = open_lock(directory)?;
    if exclusive {
        file.lock()
    } else {
        file.lock_shared()
    }
    .map_err(|error| error.to_string())?;
    Ok(file)
}

fn open_lock(directory: &Path) -> Result<File, String> {
    let namespace = directory.parent().ok_or("missing cache namespace")?;
    if !namespace
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return Err("source cache namespace is not a regular directory".into());
    }
    // Never unlink the lock: a waiting process must acquire the same inode as
    // a new reader/writer. The OS releases the lease on exit, including SIGKILL.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(namespace.with_file_name(format!(
                ".{}.lock",
                namespace
                    .file_name()
                    .ok_or("missing namespace name")?
                    .to_string_lossy()
            )))
        .map_err(|error| error.to_string())?;
    if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err("source cache lock is not a regular file".into());
    }
    Ok(file)
}

struct Generation {
    directory: PathBuf,
    published: SystemTime,
    bytes: u64,
}

fn generation(directory: PathBuf) -> Option<Generation> {
    let key = directory.file_name()?.to_str()?;
    if key.len() != 64
        || !key.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !directory.symlink_metadata().ok()?.is_dir()
    {
        return None;
    }
    let receipt_path = directory.join("receipt.json");
    let receipt: Receipt = serde_json::from_slice(&read_file(&receipt_path)?).ok()?;
    if receipt.schema != "rinzu.source-cache-v4" || receipt.key != key {
        return None;
    }
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(&directory).ok()? {
        let metadata = entry.ok()?.path().symlink_metadata().ok()?;
        // Sealed cache entries are flat, regular files. Do not traverse links
        // or unknown directory trees while deciding what this cache owns.
        if !metadata.is_file() {
            return None;
        }
        bytes = bytes.checked_add(metadata.len())?;
    }
    Some(Generation {
        directory,
        published: receipt_path.metadata().ok()?.modified().ok()?,
        bytes,
    })
}

/// Caller holds the exclusive namespace lease. In-progress generations and
/// foreign entries are never candidates. Reclaim the just-published entry last
/// if abandoned files make even that generation exceed the entire budget.
pub(super) fn collect(namespace: &Path, current: &Path, limits: Limits) -> Result<(), String> {
    let mut generations: Vec<_> = std::fs::read_dir(namespace)
        .map_err(|error| error.to_string())?
        .filter_map(|entry| generation(entry.ok()?.path()))
        .collect();
    let mut bytes: u64 = generations.iter().map(|entry| entry.bytes).sum();
    let mut count = generations.len();
    generations.sort_by(|a, b| {
        (a.directory == current, &a.published, &a.directory).cmp(&(
            b.directory == current,
            &b.published,
            &b.directory,
        ))
    });
    let mut discarded_current = false;
    for entry in generations {
        if count <= limits.generations && bytes <= limits.bytes {
            break;
        }
        std::fs::remove_dir_all(&entry.directory).map_err(|error| error.to_string())?;
        discarded_current |= entry.directory == current;
        bytes -= entry.bytes;
        count -= 1;
    }
    if discarded_current {
        Err("source cache generation with abandoned files exceeds namespace capacity".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
