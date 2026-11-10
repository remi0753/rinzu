use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::read::MultiGzDecoder;
use rinzu_compiler::modules_lock::{
    MAX_LOCK_BYTES, RECEIPT, archive_entry, archive_package_root, lexical_path, valid_sha256,
};
use rinzu_compiler::standard_library::{cache_root, sha256};
use rinzu_compiler::{ArchivePin, ArchiveReceipt};

use crate::verify_capture::TemporaryDirectory;

const MAX_DOWNLOAD: u64 = 32 * 1024 * 1024;
const MAX_EXPANDED: u64 = 128 * 1024 * 1024;
const MAX_ENTRIES: usize = 8192;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
pub(super) struct Request {
    pub name: String,
    pub locator: String,
    pub sha256: Option<String>,
    pub tree_sha256: Option<String>,
}

impl Request {
    pub fn pinned(name: &str, pin: &ArchivePin) -> Self {
        Self {
            name: name.into(),
            locator: pin.locator.clone(),
            sha256: Some(pin.sha256.clone()),
            tree_sha256: Some(pin.tree_sha256.clone()),
        }
    }
}

pub(super) struct Store<'a> {
    pub project: &'a Path,
    pub offline: bool,
}

impl Store<'_> {
    pub fn materialize(
        &self,
        requests: &[Request],
    ) -> std::result::Result<Vec<ArchivePin>, String> {
        let workers = requests.len().min(4);
        let mut results = std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..workers)
                .map(|worker| {
                    scope.spawn(move || {
                        requests
                            .iter()
                            .enumerate()
                            .skip(worker)
                            .step_by(workers)
                            .map(|(index, request)| {
                                let result = self
                                    .install(request)
                                    .map_err(|error| format!("SDK `{}`: {error}", request.name));
                                (index, result)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter()
                .flat_map(|job| job.join().expect("archive worker panicked"))
                .collect::<Vec<_>>()
        });
        results.sort_unstable_by_key(|(index, _)| *index);
        let mut pins = Vec::new();
        let mut errors = Vec::new();
        for (_, result) in results {
            match result {
                Ok(pin) => pins.push(pin),
                Err(error) => errors.push(error),
            }
        }
        if errors.is_empty() {
            Ok(pins)
        } else {
            Err(errors.join("\n"))
        }
    }

    fn install(&self, request: &Request) -> Result<ArchivePin> {
        let project_modules = self.project.join("modules");
        let mut first_download = None;
        let digest = match &request.sha256 {
            Some(digest) => digest.clone(),
            None => {
                let expected = if self.offline {
                    None
                } else {
                    github_digest(&request.locator)?
                };
                let bytes = self.download(&request.locator)?;
                let digest = sha256(&bytes);
                if let Some(expected) = expected
                    && digest != expected
                {
                    return Err("SHA-256 mismatch with GitHub Releases".into());
                }
                first_download = Some(bytes);
                digest
            }
        };
        if let Ok(pin) = installed(&project_modules, request, &digest) {
            return Ok(pin);
        }

        let cache = cache_root()?.join("modules");
        let cache_store = ensure_store(&cache)?;
        let locks = cache.join("locks");
        ensure_directory(&locks)?;
        let _guard = lock_file(&locks.join(&digest))?;
        // A concurrent installation of the same digest may have finished while we waited.
        if let Ok(pin) = installed(&project_modules, request, &digest) {
            return Ok(pin);
        }
        let pin = match installed(&cache, request, &digest) {
            Ok(pin) => pin,
            Err(_) => {
                let bytes = match first_download {
                    Some(bytes) => bytes,
                    None => self.download(&request.locator)?,
                };
                if sha256(&bytes) != digest {
                    return Err("SHA-256 mismatch with pinned archive".into());
                }
                let stage = TemporaryDirectory::in_directory(&cache_store, "sdk-extract")?;
                extract(&bytes, &stage.0, &digest)?;
                let pin = verified_entry(&stage.0, request, &digest)?;
                publish(stage, &archive_entry(&cache, &digest))?;
                pin
            }
        };

        let project_store = ensure_store(&project_modules)?;
        let stage = TemporaryDirectory::in_directory(&project_store, "sdk-copy")?;
        copy_entry(&archive_entry(&cache, &digest), &stage.0, &pin)?;
        publish(stage, &archive_entry(&project_modules, &digest))?;
        Ok(pin)
    }

    fn download(&self, locator: &str) -> Result<Vec<u8>> {
        if locator.starts_with("https://") {
            if self.offline {
                return Err("missing verified archive in offline mode".into());
            }
            fetch(locator, MAX_DOWNLOAD)
        } else {
            let path = self.project.join(locator);
            if !fs::metadata(&path)?.is_file() {
                return Err(format!("archive is not a regular file: {}", path.display()).into());
            }
            read_bounded(File::open(path)?, MAX_DOWNLOAD)
        }
    }
}

pub(super) fn lock_file(path: &Path) -> Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && !metadata.is_file()
    {
        return Err(format!("lock is not a regular file: {}", path.display()).into());
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

fn ensure_store(modules: &Path) -> Result<PathBuf> {
    ensure_directory(modules)?;
    let store = modules.join("store");
    ensure_directory(&store)?;
    let digests = store.join("sha256");
    ensure_directory(&digests)?;
    Ok(digests)
}

fn ensure_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(format!("SDK store is not a real directory: {}", path.display()).into());
    }
    Ok(())
}

fn installed(modules: &Path, request: &Request, digest: &str) -> Result<ArchivePin> {
    archive_package_root(modules, digest)?;
    verified_entry(&archive_entry(modules, digest), request, digest)
}

fn read_receipt(entry: &Path) -> Result<Vec<u8>> {
    let path = entry.join(RECEIPT);
    if !fs::symlink_metadata(&path)?.is_file() {
        return Err("archive inventory must be a regular file".into());
    }
    read_bounded(File::open(path)?, MAX_LOCK_BYTES)
}

fn verified_entry(entry: &Path, request: &Request, digest: &str) -> Result<ArchivePin> {
    let receipt = read_receipt(entry)?;
    let tree_sha256 = sha256(&receipt);
    if request
        .tree_sha256
        .as_ref()
        .is_some_and(|expected| *expected != tree_sha256)
    {
        return Err("archive file inventory does not match its lock".into());
    }
    let pin = ArchivePin {
        locator: request.locator.clone(),
        sha256: digest.into(),
        tree_sha256,
    };
    let root = entry.join("package");
    for (relative, expected) in ArchiveReceipt::parse(&receipt, &pin)?.files {
        let path = root.join(relative);
        if !fs::symlink_metadata(&path)?.is_file()
            || path.canonicalize()? != lexical_path(&path)
            || sha256(&read_bounded(File::open(&path)?, MAX_EXPANDED)?) != expected
        {
            return Err(format!(
                "SDK source differs from its verified inventory: {}",
                path.display()
            )
            .into());
        }
    }
    Ok(pin)
}

fn copy_entry(source: &Path, destination: &Path, pin: &ArchivePin) -> Result<()> {
    let receipt = read_receipt(source)?;
    for relative in ArchiveReceipt::parse(&receipt, pin)?.files.keys() {
        let from = source.join("package").join(relative);
        let to = destination.join("package").join(relative);
        fs::create_dir_all(to.parent().expect("inventory file has a parent"))?;
        fs::copy(from, to)?;
    }
    fs::write(destination.join(RECEIPT), receipt)?;
    Ok(())
}

fn publish(stage: TemporaryDirectory, destination: &Path) -> Result<()> {
    // The hash lock serializes replacements. A corrupt entry is kept until its
    // replacement is ready, so other hashes and the project's pins are unaffected.
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() => {
            let previous =
                TemporaryDirectory::in_directory(destination.parent().unwrap(), "sdk-repair")?;
            let old = previous.0.join("entry");
            fs::rename(destination, &old)?;
            if let Err(error) = fs::rename(&stage.0, destination) {
                if let Err(restore) = fs::rename(&old, destination) {
                    std::mem::forget(previous);
                    return Err(format!(
                        "cannot install SDK: {error}; cannot restore {}: {restore}",
                        old.display()
                    )
                    .into());
                }
                return Err(error.into());
            }
        }
        Ok(_) => return Err("SDK store entry is not a real directory".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::rename(&stage.0, destination)?,
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn read_bounded(reader: impl Read, maximum: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err("archive or metadata exceeds its size limit".into());
    }
    Ok(bytes)
}

fn fetch(url: &str, maximum: u64) -> Result<Vec<u8>> {
    let mut child = Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--globoff",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "5",
            "--connect-timeout",
            "10",
            "--max-time",
            "90",
            "--url",
            url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start curl: {error}"))?;
    let result = read_bounded(child.stdout.take().expect("piped curl stdout"), maximum);
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let bytes = result?;
    if !status.success() {
        return Err(format!("HTTPS download failed ({status}): {url}").into());
    }
    Ok(bytes)
}

fn github_digest(url: &str) -> Result<Option<String>> {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    let Some(path) = base.strip_prefix("https://github.com/") else {
        return Ok(None);
    };
    let parts: Vec<_> = path.split('/').collect();
    let [owner, repo, "releases", "download", tag, asset] = parts.as_slice() else {
        return Ok(None);
    };
    if [owner, repo, tag, asset]
        .iter()
        .any(|part| part.is_empty() || matches!(**part, "." | ".."))
    {
        return Err("invalid GitHub Releases asset URL".into());
    }
    let api = format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}");
    let release: serde_json::Value = serde_json::from_slice(&fetch(&api, 4 * 1024 * 1024)?)?;
    let digest = release.get("assets").and_then(serde_json::Value::as_array)
        .and_then(|assets| assets.iter().find(|candidate| {
            let Some(link) = candidate.get("browser_download_url").and_then(serde_json::Value::as_str) else { return false };
            let Some(link) = link.strip_prefix("https://github.com/") else { return false };
            let fields: Vec<_> = link.split('/').collect();
            matches!(fields.as_slice(), [o, r, "releases", "download", t, a]
                if o.eq_ignore_ascii_case(owner) && r.eq_ignore_ascii_case(repo) && t == tag && a == asset)
        }))
        .and_then(|asset| asset.get("digest"))
        .and_then(serde_json::Value::as_str)
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .filter(|digest| valid_sha256(digest))
        .ok_or("GitHub Releases did not provide a SHA-256 digest for this asset")?;
    Ok(Some(digest.to_owned()))
}

fn extract(bytes: &[u8], stage: &Path, digest: &str) -> Result<()> {
    let tar_path = stage.join("archive.tar");
    let mut tar = File::create(&tar_path)?;
    if io::copy(
        &mut MultiGzDecoder::new(bytes).take(MAX_EXPANDED + 1),
        &mut tar,
    )? > MAX_EXPANDED
    {
        return Err("expanded SDK archive exceeds its size limit".into());
    }
    drop(tar);
    let unpacked = stage.join("unpacked");
    fs::create_dir(&unpacked)?;
    let mut archive = tar::Archive::new(File::open(&tar_path)?);
    let mut seen = BTreeSet::new();
    let mut files = BTreeMap::new();
    for (index, entry) in archive.entries()?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err("SDK archive contains too many entries".into());
        }
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            continue;
        }
        if !kind.is_file() && !kind.is_dir() {
            return Err("SDK archive must contain only regular files and directories".into());
        }
        let path = entry.path()?;
        let mut relative = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(name)
                    if name.to_str().is_some_and(|name| {
                        !name
                            .chars()
                            .any(|character| character.is_control() || character == '\\')
                    }) =>
                {
                    relative.push(name)
                }
                Component::CurDir => {}
                _ => return Err("SDK archive contains an unsafe path".into()),
            }
        }
        if relative.as_os_str().is_empty() && kind.is_dir() {
            continue;
        }
        if relative.as_os_str().is_empty()
            || relative.components().count() > 64
            || !seen.insert(relative.clone())
        {
            return Err("SDK archive contains a duplicate or invalid path".into());
        }
        let target = unpacked.join(&relative);
        if kind.is_dir() {
            fs::create_dir_all(target)?;
        } else {
            fs::create_dir_all(target.parent().unwrap())?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)?;
            if io::copy(&mut entry, &mut file)? != entry.size() {
                return Err("truncated SDK archive file".into());
            }
            files.insert(relative, sha256(&fs::read(&target)?));
        }
    }
    drop(archive);
    fs::remove_file(tar_path)?;
    let root = if unpacked.join("rinzu.toml").is_file() {
        unpacked.clone()
    } else {
        let mut entries = fs::read_dir(&unpacked)?;
        let first = entries.next().transpose()?.ok_or("empty SDK archive")?;
        if entries.next().is_some()
            || !first.file_type()?.is_dir()
            || !first.path().join("rinzu.toml").is_file()
        {
            return Err(
                "SDK archive needs rinzu.toml at its root or in a single enclosing directory"
                    .into(),
            );
        }
        first.path()
    };
    let prefix = root.strip_prefix(&unpacked)?;
    let receipt = ArchiveReceipt {
        archive_sha256: digest.into(),
        files: files
            .into_iter()
            .map(|(path, hash)| {
                (
                    path.strip_prefix(prefix)
                        .expect("SDK archive has one package root")
                        .to_owned(),
                    hash,
                )
            })
            .collect(),
    };
    fs::rename(&root, stage.join("package"))?;
    if unpacked.exists() {
        fs::remove_dir(unpacked)?;
    }
    fs::write(stage.join(RECEIPT), serde_json::to_vec(&receipt)?)?;
    Ok(())
}
