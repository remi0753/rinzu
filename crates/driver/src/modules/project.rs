//! Recovery for the two files that make a project dependency change visible.
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rinzu_compiler::modules_lock::{FILE, MAX_LOCK_BYTES};
use rinzu_compiler::standard_library::sha256;
use serde::{Deserialize, Serialize};

use crate::verify_capture::TemporaryDirectory;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
const MANIFEST_LIMIT: u64 = 1024 * 1024;
const JOURNAL_LIMIT: u64 = 2 * (MAX_LOCK_BYTES + MANIFEST_LIMIT) + 1024;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub manifest: String,
    pub lock: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    before: [Option<String>; 2],
    after: State,
}

impl State {
    pub fn read(root: &Path) -> Result<Self> {
        Ok(Self {
            manifest: read(&root.join("rinzu.toml"), MANIFEST_LIMIT)?
                .ok_or("missing rinzu.toml")?,
            lock: read(&root.join(FILE), MAX_LOCK_BYTES)?,
        })
    }

    fn digests(&self) -> [Option<String>; 2] {
        [
            Some(sha256(self.manifest.as_bytes())),
            self.lock.as_ref().map(|lock| sha256(lock.as_bytes())),
        ]
    }
}

pub(super) fn guard(root: &Path) -> Result<File> {
    let directory = File::open(root)?;
    directory.lock()?;
    Ok(directory)
}

pub(super) fn snapshot(root: &Path) -> Result<State> {
    let _guard = guard(root)?;
    recover(root)?;
    State::read(root)
}

pub(super) fn update(root: &Path, before: &State, after: State) -> Result<()> {
    let _guard = guard(root)?;
    recover(root)?;
    if &State::read(root)? != before {
        return Err(
            "the manifest or modules lock changed during acquisition; retry the command".into(),
        );
    }
    let journal = journal_path(root)?;
    match fs::create_dir(journal.parent().unwrap()) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let transaction = Transaction {
        before: before.digests(),
        after,
    };
    let bytes = serde_json::to_vec(&transaction)?;
    if bytes.len() as u64 > JOURNAL_LIMIT {
        return Err("modules transaction exceeds its size limit".into());
    }
    write_atomic(&journal, &bytes)?;
    finish(root, &journal, transaction)
}

fn journal_path(root: &Path) -> Result<PathBuf> {
    let directory = root.join(".rinzu");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(".rinzu must be a real project directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(directory.join("modules-transaction.json"))
}

fn recover(root: &Path) -> Result<()> {
    let journal = journal_path(root)?;
    if let Some(bytes) = read(&journal, JOURNAL_LIMIT)? {
        finish(root, &journal, serde_json::from_str(&bytes)?)?;
    }
    Ok(())
}

fn finish(root: &Path, journal: &Path, transaction: Transaction) -> Result<()> {
    let current = State::read(root)?;
    let expected = transaction.after.digests();
    if current
        .digests()
        .iter()
        .enumerate()
        .any(|(index, digest)| *digest != transaction.before[index] && *digest != expected[index])
    {
        return Err("manifest or lock was edited during an interrupted modules transaction".into());
    }
    let lock = transaction
        .after
        .lock
        .as_ref()
        .ok_or("transaction missing its modules lock")?;
    if current.lock.as_ref() != Some(lock) {
        write_atomic(&root.join(FILE), lock.as_bytes())?;
    }
    if current.manifest != transaction.after.manifest {
        write_atomic(
            &root.join("rinzu.toml"),
            transaction.after.manifest.as_bytes(),
        )?;
    }
    fs::remove_file(journal)?;
    Ok(())
}

fn read(path: &Path, maximum: u64) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= maximum => {}
        Ok(_) => return Err(format!("invalid metadata file: {}", path.display()).into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let mut result = String::new();
    File::open(path)?
        .take(maximum + 1)
        .read_to_string(&mut result)?;
    if result.len() as u64 > maximum {
        return Err(format!("metadata file exceeds its limit: {}", path.display()).into());
    }
    Ok(Some(result))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let stage = TemporaryDirectory::in_directory(path.parent().unwrap(), "modules-write")?;
    let staged = stage.0.join("metadata");
    let mut file = File::create(&staged)?;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file() {
            return Err(format!("metadata path is not a regular file: {}", path.display()).into());
        }
        file.set_permissions(metadata.permissions())?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staged, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_interrupted_pair_and_respects_an_external_edit() {
        let scratch = TemporaryDirectory::new("modules-transaction-test").unwrap();
        let root = &scratch.0;
        let path = root.join("rinzu.toml");
        fs::write(&path, "old manifest").unwrap();
        let before = State::read(root).unwrap();
        let after = State {
            manifest: "new manifest".into(),
            lock: Some("new lock".into()),
        };
        let journal = journal_path(root).unwrap();
        fs::create_dir(journal.parent().unwrap()).unwrap();
        let transaction = Transaction {
            before: before.digests(),
            after,
        };
        let bytes = serde_json::to_vec(&transaction).unwrap();
        fs::write(&journal, &bytes).unwrap();
        fs::write(root.join(FILE), "new lock").unwrap();
        assert_eq!(snapshot(root).unwrap(), transaction.after);
        assert!(!journal.exists());

        fs::write(&journal, bytes).unwrap();
        fs::write(&path, "external edit").unwrap();
        assert!(snapshot(root).unwrap_err().to_string().contains("edited"));
        assert_eq!(fs::read_to_string(path).unwrap(), "external edit");
    }
}
