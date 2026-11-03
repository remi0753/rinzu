//! Source snapshots shared by disk package checking and editor overlays.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{Diagnostic, FileId, Module, SourceFile};

type ParseResult = Result<Module, Vec<Diagnostic>>;
type CachedParse = (Vec<u8>, ParseResult);

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

pub struct SourceSnapshot<'a> {
    cache: &'a mut SourceCache,
    overlays: BTreeMap<PathBuf, Vec<u8>>,
    resolved_paths: RefCell<BTreeMap<PathBuf, PathBuf>>,
    reads: BTreeMap<PathBuf, Vec<u8>>,
    config_reads: BTreeMap<PathBuf, Vec<u8>>,
    native_headers: BTreeSet<PathBuf>,
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
            reads: BTreeMap::new(),
            config_reads: BTreeMap::new(),
            native_headers: BTreeSet::new(),
            sources: BTreeMap::new(),
            parse_hits: 0,
            parse_misses: 0,
            limits: None,
        }
    }

    pub fn set_limits(&mut self, maximum_bytes: usize, maximum_files: usize) {
        self.limits = Some((maximum_bytes, maximum_files));
    }

    #[must_use]
    pub fn has_overlay(&self, canonical_path: &Path) -> bool {
        self.overlays.contains_key(canonical_path)
    }

    pub fn canonical_path(&self, path: &Path) -> io::Result<PathBuf> {
        match path.canonicalize() {
            Ok(canonical) => {
                self.resolved_paths
                    .borrow_mut()
                    .entry(path.to_owned())
                    .or_insert_with(|| canonical.clone());
                Ok(canonical)
            }
            Err(original) => {
                let parent = path
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                let candidate = parent
                    .canonicalize()?
                    .join(path.file_name().ok_or(original.kind())?);
                if self.overlays.contains_key(&candidate) {
                    Ok(candidate)
                } else {
                    Err(original)
                }
            }
        }
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
        self.reads.insert(path.to_owned(), bytes.clone());
        Ok(bytes)
    }

    /// Configuration is disk-only, pinned separately from source overlays.
    pub(crate) fn read_config(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let path = path.canonicalize()?;
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
        self.config_reads.insert(path, bytes.clone());
        Ok(bytes)
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

    pub fn source(&mut self, path: &Path, bytes: Vec<u8>) -> Result<SourceFile, Vec<Diagnostic>> {
        static NEXT_FILE: AtomicU32 = AtomicU32::new(1);
        let id = *self
            .cache
            .ids
            .entry(path.to_owned())
            .or_insert_with(|| FileId(NEXT_FILE.fetch_add(1, Ordering::Relaxed)));
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

    #[must_use]
    pub fn disk_is_current(&self) -> bool {
        self.resolved_paths
            .borrow()
            .iter()
            .all(|(path, canonical)| path.canonicalize().is_ok_and(|now| now == *canonical))
            && self.reads.iter().all(|(path, bytes)| {
                self.overlays.contains_key(path)
                    || std::fs::read(path).is_ok_and(|now| now == *bytes)
            })
            && self
                .config_reads
                .iter()
                .all(|(path, bytes)| std::fs::read(path).is_ok_and(|now| now == *bytes))
    }
}
