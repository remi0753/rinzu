//! Source snapshots shared by disk package checking and editor overlays.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{ArchiveReceipt, Diagnostic, FileId, Module, SourceFile};

type ParseResult = Result<Module, Vec<Diagnostic>>;
type CachedParse = (Vec<u8>, ParseResult);
type SourceEntries = Vec<(PathBuf, bool)>;

#[derive(Default)]
pub struct SourceCache {
    ids: BTreeMap<PathBuf, FileId>,
    parsed: BTreeMap<PathBuf, CachedParse>,
}

impl SourceCache {
    pub fn retain_paths(&mut self, paths: &BTreeSet<PathBuf>) {
        self.ids.retain(|path, _| paths.contains(path));
        self.parsed.retain(|path, _| paths.contains(path));
    }
}

/// Bounded, exact evidence for reuse of one editor analysis.
pub struct SourceFreshness {
    resolved_paths: BTreeMap<PathBuf, Option<PathBuf>>,
    disk: BTreeMap<PathBuf, Vec<u8>>,
    source_directories: BTreeMap<PathBuf, SourceEntries>,
}

impl SourceFreshness {
    pub fn disk_is_current(&self) -> bool {
        paths_current(&self.resolved_paths)
            && self.disk.iter().all(file_current)
            && directories_current(&self.source_directories)
    }
}

fn paths_current(paths: &BTreeMap<PathBuf, Option<PathBuf>>) -> bool {
    paths
        .iter()
        .all(|(path, resolved)| path.canonicalize().ok() == *resolved)
}

fn file_current((path, bytes): (&PathBuf, &Vec<u8>)) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == bytes.len() as u64)
        && std::fs::read(path).is_ok_and(|now| now == *bytes)
}

fn source_entries(directory: &Path) -> io::Result<SourceEntries> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        let directory = kind.is_dir() || (kind.is_symlink() && path.is_dir());
        if directory
            || path
                .extension()
                .is_some_and(|extension| extension == "rinzu")
        {
            entries.push((path, directory));
        }
    }
    entries.sort();
    Ok(entries)
}

fn directories_current(directories: &BTreeMap<PathBuf, SourceEntries>) -> bool {
    directories.iter().all(|(directory, entries)| {
        source_entries(directory).is_ok_and(|current| current == *entries)
    })
}

pub struct SourceSnapshot<'a> {
    cache: &'a mut SourceCache,
    overlays: BTreeMap<PathBuf, Vec<u8>>,
    resolved_paths: RefCell<BTreeMap<PathBuf, Option<PathBuf>>>,
    editor_file: Option<PathBuf>,
    source_directories: BTreeMap<PathBuf, SourceEntries>,
    reads: BTreeMap<PathBuf, Vec<u8>>,
    config_reads: BTreeMap<PathBuf, Vec<u8>>,
    native_headers: BTreeSet<PathBuf>,
    archives: BTreeMap<PathBuf, ArchiveReceipt>,
    archive_directories: BTreeMap<PathBuf, BTreeMap<PathBuf, bool>>,
    sources: BTreeMap<PathBuf, SourceFile>,
    pub parse_hits: usize,
    pub parse_misses: usize,
    limits: Option<(usize, usize)>,
}

impl<'a> SourceSnapshot<'a> {
    pub fn new(cache: &'a mut SourceCache, overlays: BTreeMap<PathBuf, Vec<u8>>) -> Self {
        Self {
            cache,
            overlays,
            resolved_paths: RefCell::new(BTreeMap::new()),
            editor_file: None,
            source_directories: BTreeMap::new(),
            reads: BTreeMap::new(),
            config_reads: BTreeMap::new(),
            native_headers: BTreeSet::new(),
            archives: BTreeMap::new(),
            archive_directories: BTreeMap::new(),
            sources: BTreeMap::new(),
            parse_hits: 0,
            parse_misses: 0,
            limits: None,
        }
    }

    pub fn set_editor_file(&mut self, path: PathBuf) {
        self.editor_file = Some(path);
    }

    pub(crate) fn editor_file(&self) -> Option<FileId> {
        self.sources
            .get(self.editor_file.as_ref()?)
            .map(SourceFile::id)
    }

    pub fn set_limits(&mut self, maximum_bytes: usize, maximum_files: usize) {
        self.limits = Some((maximum_bytes, maximum_files));
    }

    #[must_use]
    pub fn has_overlay(&self, canonical_path: &Path) -> bool {
        self.overlays.contains_key(canonical_path)
    }

    pub fn canonical_path(&self, path: &Path) -> io::Result<PathBuf> {
        let archived = self.archive_for(path).is_some();
        match path.canonicalize() {
            Ok(canonical) => {
                if archived && canonical != crate::modules_lock::lexical_path(path) {
                    return Err(io::Error::other(
                        "verified SDK paths must not traverse symlinks",
                    ));
                }
                self.resolved_paths
                    .borrow_mut()
                    .entry(path.to_owned())
                    .or_insert_with(|| Some(canonical.clone()));
                Ok(canonical)
            }
            Err(original) if archived => Err(original),
            Err(original) => {
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let resolved_parent = parent.canonicalize()?;
                let candidate = resolved_parent.join(path.file_name().ok_or(original.kind())?);
                if self.overlays.contains_key(&candidate) {
                    let mut resolutions = self.resolved_paths.borrow_mut();
                    resolutions
                        .entry(parent.to_owned())
                        .or_insert(Some(resolved_parent));
                    resolutions.entry(path.to_owned()).or_insert(None);
                    Ok(candidate)
                } else {
                    Err(original)
                }
            }
        }
    }

    pub(crate) fn source_entries(&mut self, directory: &Path) -> io::Result<SourceEntries> {
        if self.archive_for(directory).is_some() {
            return Ok(self
                .archive_directories
                .get(directory)
                .into_iter()
                .flatten()
                .map(|(path, directory)| (path.clone(), *directory))
                .collect());
        }
        let entries = source_entries(directory)?;
        if self.editor_file.is_some() {
            self.source_directories
                .entry(directory.to_owned())
                .or_insert_with(|| entries.clone());
        }
        Ok(entries)
    }

    pub fn read_bytes(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.reads.get(path) {
            return Ok(bytes.clone());
        }
        if let Some((_, maximum_files)) = self.limits
            && self.reads.len() + self.config_reads.len() >= maximum_files
        {
            return Err(io::Error::other("source snapshot exceeds its file limit"));
        }
        let bytes = if let Some(bytes) = self.overlays.get(path) {
            bytes.clone()
        } else {
            if let Some((maximum_bytes, _)) = self.limits
                && std::fs::metadata(path)?.len() > maximum_bytes as u64
            {
                return Err(io::Error::other("source exceeds the editor document limit"));
            }
            std::fs::read(path)?
        };
        if let Some((maximum_bytes, _)) = self.limits
            && bytes.len() > maximum_bytes
        {
            return Err(io::Error::other("source exceeds the editor document limit"));
        }
        self.verify_archive_bytes(path, &bytes)?;
        self.reads.insert(path.to_owned(), bytes.clone());
        Ok(bytes)
    }

    /// Configuration is disk-only, pinned separately from source overlays.
    pub(crate) fn read_config(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let path = self.canonical_path(path)?;
        if let Some(bytes) = self.config_reads.get(&path) {
            return Ok(bytes.clone());
        }
        if let Some((_, maximum_files)) = self.limits
            && self.reads.len() + self.config_reads.len() >= maximum_files
        {
            return Err(io::Error::other("source snapshot exceeds its file limit"));
        }
        if self.limits.is_some() && std::fs::metadata(&path)?.len() > 1024 * 1024 {
            return Err(io::Error::other(
                "manifest exceeds the editor configuration limit",
            ));
        }
        let bytes = std::fs::read(&path)?;
        if self.limits.is_some() && bytes.len() > 1024 * 1024 {
            return Err(io::Error::other(
                "manifest exceeds the editor configuration limit",
            ));
        }
        self.verify_archive_bytes(&path, &bytes)?;
        self.config_reads.insert(path, bytes.clone());
        Ok(bytes)
    }

    pub(crate) fn register_archive(&mut self, root: PathBuf, receipt: ArchiveReceipt) {
        for relative in receipt.files.keys() {
            let file = root.join(relative);
            for (child, directory) in file.ancestors().zip(file.ancestors().skip(1)) {
                if !directory.starts_with(&root) {
                    break;
                }
                let is_directory = child != file;
                if is_directory
                    || child
                        .extension()
                        .is_some_and(|extension| extension == "rinzu")
                {
                    self.archive_directories
                        .entry(directory.to_owned())
                        .or_default()
                        .insert(child.to_owned(), is_directory);
                }
            }
        }
        self.archives.insert(root, receipt);
    }

    pub(crate) fn is_archive(&self, root: &Path) -> bool {
        self.archives.contains_key(root)
    }

    fn archive_for(&self, path: &Path) -> Option<(&Path, &ArchiveReceipt)> {
        path.ancestors().find_map(|root| {
            self.archives
                .get_key_value(root)
                .map(|(root, receipt)| (root.as_path(), receipt))
        })
    }

    fn verify_archive_bytes(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some((root, receipt)) = self.archive_for(path) {
            let expected = receipt
                .files
                .get(path.strip_prefix(root).expect("archive ancestor"));
            if !expected.is_some_and(|digest| *digest == crate::standard_library::sha256(bytes)) {
                return Err(io::Error::other(
                    "SDK source differs from its verified inventory; run `rinzu modules sync`",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn read_native_header(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let canonical = self.canonical_path(path)?;
        let bytes = self.read_config(&canonical)?;
        self.native_headers.insert(canonical);
        Ok(bytes)
    }

    /// Content receipts cover only files actually consumed by this checker.
    /// The driver hashes these exact checked bytes; it must not reread an unchecked generation.
    #[must_use]
    pub fn input_bytes(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        self.reads
            .iter()
            .chain(&self.config_reads)
            .map(|(path, bytes)| (path.clone(), bytes.clone()))
            .collect()
    }

    /// Content receipts cover only files actually consumed by this checker.
    #[must_use]
    pub fn input_receipts(&self) -> Vec<(PathBuf, &'static str, usize, u64)> {
        self.reads
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    if self.overlays.contains_key(path) {
                        "overlay"
                    } else {
                        "disk-source"
                    },
                    bytes.len(),
                    crate::fnv1a64(bytes),
                )
            })
            .chain(self.config_reads.iter().map(|(path, bytes)| {
                (
                    path.clone(),
                    if self.native_headers.contains(path) {
                        "disk-native-header"
                    } else {
                        "disk-manifest"
                    },
                    bytes.len(),
                    crate::fnv1a64(bytes),
                )
            }))
            .collect()
    }

    pub(crate) fn file_id(&mut self, path: &Path) -> FileId {
        static NEXT_FILE: AtomicU32 = AtomicU32::new(1);
        *self
            .cache
            .ids
            .entry(path.to_owned())
            .or_insert_with(|| FileId(NEXT_FILE.fetch_add(1, Ordering::Relaxed)))
    }

    pub fn source(&mut self, path: &Path, bytes: Vec<u8>) -> Result<SourceFile, Vec<Diagnostic>> {
        let id = self.file_id(path);
        let source = SourceFile::from_utf8(id, path, bytes)?;
        self.sources.insert(path.to_owned(), source.clone());
        Ok(source)
    }

    pub fn parse(&mut self, file: &SourceFile) -> Result<Module, Vec<Diagnostic>> {
        let bytes = self
            .reads
            .get(file.path())
            .expect("snapshot source read first");
        if let Some((previous, result)) = self.cache.parsed.get(file.path())
            && previous == bytes
        {
            self.parse_hits += 1;
            return result.clone();
        }
        self.parse_misses += 1;
        let _timing = crate::timing::span("frontend.parse")
            .detail(file.path().to_str().unwrap_or("<non-UTF-8 source path>"));
        let result = crate::parse_source(file);
        self.cache
            .parsed
            .insert(file.path().to_owned(), (bytes.clone(), result.clone()));
        result
    }

    #[must_use]
    pub fn sources(&self) -> &BTreeMap<PathBuf, SourceFile> {
        &self.sources
    }

    #[must_use]
    pub fn path_for_id(&self, id: FileId) -> Option<&Path> {
        self.cache
            .ids
            .iter()
            .find_map(|(path, candidate)| (*candidate == id).then_some(path.as_path()))
    }

    #[must_use]
    pub fn bytes_for_path(&self, path: &Path) -> Option<&[u8]> {
        self.reads
            .get(path)
            .or_else(|| self.config_reads.get(path))
            .map(Vec::as_slice)
    }

    #[must_use]
    pub fn parsed_modules(&self) -> Vec<(SourceFile, Module)> {
        self.sources
            .iter()
            .filter_map(|(path, source)| {
                self.cache
                    .parsed
                    .get(path)
                    .and_then(|(_, result)| result.as_ref().ok())
                    .map(|module| (source.clone(), module.clone()))
            })
            .collect()
    }

    #[must_use]
    pub fn read_paths(&self) -> BTreeSet<PathBuf> {
        self.reads.keys().cloned().collect()
    }

    fn disk_reads(&self) -> impl Iterator<Item = (&PathBuf, &Vec<u8>)> {
        self.reads
            .iter()
            .filter(|(path, _)| !self.overlays.contains_key(*path))
            .chain(&self.config_reads)
    }

    pub fn freshness(&self, maximum_bytes: usize) -> Option<SourceFreshness> {
        let bytes = self
            .disk_reads()
            .map(|(_, bytes)| bytes.len())
            .sum::<usize>();
        let directory_entries = self
            .source_directories
            .values()
            .map(Vec::len)
            .sum::<usize>();
        if bytes > maximum_bytes
            || directory_entries > 1024
            || self.resolved_paths.borrow().len() > 1024
        {
            return None;
        }
        Some(SourceFreshness {
            resolved_paths: self.resolved_paths.borrow().clone(),
            source_directories: self.source_directories.clone(),
            disk: self
                .disk_reads()
                .map(|(path, bytes)| (path.clone(), bytes.clone()))
                .collect(),
        })
    }

    #[must_use]
    pub fn disk_is_current(&self) -> bool {
        paths_current(&self.resolved_paths.borrow())
            && self.disk_reads().all(file_current)
            && directories_current(&self.source_directories)
    }
}
