//! Source snapshots shared by disk package checking and editor overlays.
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{
    ArchiveReceipt, CheckedModule, CompilerOptions, Diagnostic, FileId, Module, SdkFrontendCache,
    SourceFile, SourceSdkInterface,
};

type ParseResult = Result<Module, Vec<Diagnostic>>;
type CachedParse = (Vec<u8>, ParseResult);
type SourceEntries = Vec<(PathBuf, bool)>;

/// Frontend work reported as it starts, independently of CLI output policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontendProgress<'a> {
    ResolvingDependencies,
    CheckingDependency(&'a str),
    ParsingApplicationSources,
    CheckingApplicationTypes,
    PreparingAndVerifyingIr,
}

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

/// Files are carried by the checked artifact. This records how those files
/// were found and the directory membership tests the checker actually used.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DiskSourceEvidence {
    resolved_paths: BTreeMap<PathBuf, Option<PathBuf>>,
    path_kinds: BTreeMap<PathBuf, Option<DiskPathKind>>,
    source_directories: BTreeMap<PathBuf, SourceEntries>,
    archives: Vec<ArchiveLocation>,
    regular_inputs: BTreeSet<PathBuf>,
    native_trees: Vec<NativeTree>,
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
enum DiskPathKind {
    File,
    Directory,
    Other,
}

fn disk_path_kind(path: &Path) -> Option<DiskPathKind> {
    let metadata = path.metadata().ok()?;
    Some(if metadata.is_dir() {
        DiskPathKind::Directory
    } else if metadata.is_file() {
        DiskPathKind::File
    } else {
        DiskPathKind::Other
    })
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ArchiveLocation {
    modules: PathBuf,
    digest: String,
    root: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NativeTree {
    root: PathBuf,
    archive: bool,
    files: BTreeMap<PathBuf, String>,
}

impl DiskSourceEvidence {
    pub fn disk_is_current(&self, checked_bytes: &BTreeMap<PathBuf, Vec<u8>>) -> bool {
        paths_current(&self.resolved_paths)
            && self
                .path_kinds
                .iter()
                .all(|(path, expected)| disk_path_kind(path) == *expected)
            && self.archives.iter().all(|archive| {
                crate::modules_lock::archive_package_root(&archive.modules, &archive.digest)
                    .is_ok_and(|root| root == archive.root)
            })
            && self.regular_inputs.iter().all(|path| {
                std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
            })
            && checked_bytes.iter().all(file_current)
            && directories_current(&self.source_directories)
            && self.native_trees.iter().all(|tree| {
                if tree.archive {
                    crate::modules_lock::check_archive_inventory(&tree.root, &tree.files).is_ok()
                } else {
                    crate::official_sdk::check_native_files(
                        &tree.root,
                        Path::new("native"),
                        &tree.files,
                    )
                    .is_ok()
                }
            })
    }
}

fn paths_current(paths: &BTreeMap<PathBuf, Option<PathBuf>>) -> bool {
    paths
        .iter()
        .all(|(path, resolved)| path.canonicalize().ok() == *resolved)
}

fn file_current((path, bytes): (&PathBuf, &Vec<u8>)) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == bytes.len() as u64)
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

struct VerifiedPackage {
    files: BTreeMap<PathBuf, String>,
    archive: bool,
}

pub struct SourceSnapshot<'a> {
    cache: &'a mut SourceCache,
    overlays: BTreeMap<PathBuf, Vec<u8>>,
    resolved_paths: RefCell<BTreeMap<PathBuf, Option<PathBuf>>>,
    editor_file: Option<PathBuf>,
    hover_only: bool,
    source_directories: BTreeMap<PathBuf, SourceEntries>,
    reads: BTreeMap<PathBuf, Vec<u8>>,
    config_reads: BTreeMap<PathBuf, Vec<u8>>,
    native_headers: BTreeSet<PathBuf>,
    verified: BTreeMap<PathBuf, VerifiedPackage>,
    verified_directories: BTreeMap<PathBuf, BTreeMap<PathBuf, bool>>,
    sources: BTreeMap<PathBuf, SourceFile>,
    pub parse_hits: usize,
    pub parse_misses: usize,
    limits: Option<(usize, usize)>,
    track_disk: bool,
    sdk_frontend_cache: Option<&'a mut dyn SdkFrontendCache>,
    function_frontend_cache: Option<&'a mut crate::FunctionFrontendCache>,
    progress: Option<&'a mut dyn FnMut(FrontendProgress<'_>)>,
    path_kinds: RefCell<BTreeMap<PathBuf, Option<DiskPathKind>>>,
    archive_locations: Vec<ArchiveLocation>,
    regular_inputs: BTreeSet<PathBuf>,
    native_trees: BTreeSet<PathBuf>,
}

impl<'a> SourceSnapshot<'a> {
    pub fn new(cache: &'a mut SourceCache, overlays: BTreeMap<PathBuf, Vec<u8>>) -> Self {
        Self {
            cache,
            overlays,
            resolved_paths: RefCell::new(BTreeMap::new()),
            editor_file: None,
            hover_only: false,
            source_directories: BTreeMap::new(),
            reads: BTreeMap::new(),
            config_reads: BTreeMap::new(),
            native_headers: BTreeSet::new(),
            verified: BTreeMap::new(),
            verified_directories: BTreeMap::new(),
            sources: BTreeMap::new(),
            parse_hits: 0,
            parse_misses: 0,
            limits: None,
            track_disk: false,
            sdk_frontend_cache: None,
            function_frontend_cache: None,
            progress: None,
            path_kinds: RefCell::new(BTreeMap::new()),
            archive_locations: Vec::new(),
            regular_inputs: BTreeSet::new(),
            native_trees: BTreeSet::new(),
        }
    }

    pub fn track_disk_compilation(&mut self) {
        self.track_disk = true;
    }

    /// Observe only work performed by this snapshot. No observer is installed
    /// for ordinary checking, editor analysis, or formatting by default.
    pub fn set_progress_observer(&mut self, observe: &'a mut dyn FnMut(FrontendProgress<'_>)) {
        self.progress = Some(observe);
    }

    pub(crate) fn report_progress(&mut self, event: FrontendProgress<'_>) {
        if let Some(observe) = &mut self.progress {
            observe(event);
        }
    }

    pub fn set_sdk_frontend_cache(&mut self, cache: &'a mut dyn SdkFrontendCache) {
        self.sdk_frontend_cache = Some(cache);
    }

    pub fn set_function_frontend_cache(&mut self, cache: &'a mut crate::FunctionFrontendCache) {
        self.function_frontend_cache = Some(cache);
    }

    pub(crate) fn function_frontend_cache(&mut self) -> Option<&mut crate::FunctionFrontendCache> {
        if self.editor_file.is_none() && self.overlays.is_empty() {
            self.function_frontend_cache.as_deref_mut()
        } else {
            None
        }
    }

    pub(crate) fn has_sdk_frontend_cache(&self) -> bool {
        self.sdk_frontend_cache.is_some() && self.editor_file.is_none() && self.overlays.is_empty()
    }

    pub(crate) fn load_sdk_frontend(
        &mut self,
        key: &str,
        name: &str,
    ) -> Option<SourceSdkInterface> {
        self.sdk_frontend_cache.as_mut()?.load(key, name)
    }

    pub(crate) fn stage_sdk_frontend(
        &mut self,
        key: &str,
        name: &str,
        source: Arc<SourceSdkInterface>,
    ) {
        if let Some(cache) = &mut self.sdk_frontend_cache {
            cache.stage(key, name, source);
        }
    }

    pub fn disk_compilation_evidence(&self) -> Option<DiskSourceEvidence> {
        (self.track_disk && self.overlays.is_empty() && self.editor_file.is_none()).then(|| {
            DiskSourceEvidence {
                resolved_paths: self.resolved_paths.borrow().clone(),
                path_kinds: self.path_kinds.borrow().clone(),
                source_directories: self.source_directories.clone(),
                archives: self.archive_locations.clone(),
                regular_inputs: self.regular_inputs.clone(),
                native_trees: self
                    .native_trees
                    .iter()
                    .map(|root| {
                        let inventory = &self.verified[root];
                        NativeTree {
                            root: root.clone(),
                            archive: inventory.archive,
                            files: inventory.files.clone(),
                        }
                    })
                    .collect(),
            }
        })
    }

    pub(crate) fn record_archive_location(&mut self, modules: &Path, digest: &str, root: &Path) {
        if self.track_disk {
            self.archive_locations.push(ArchiveLocation {
                modules: modules.to_owned(),
                digest: digest.to_owned(),
                root: root.to_owned(),
            });
        }
    }

    pub(crate) fn record_regular_input(&mut self, path: &Path) {
        if self.track_disk {
            self.regular_inputs.insert(path.to_owned());
        }
    }

    pub(crate) fn record_native_tree(&mut self, root: &Path) {
        if self.track_disk {
            self.native_trees.insert(root.to_owned());
        }
    }

    pub fn set_editor_file(&mut self, path: PathBuf) {
        self.editor_file = Some(path);
        self.hover_only = false;
    }

    /// Collect checked hover types and Error bounds without the separate
    /// observation validation pass. Use this snapshot only for editor hover.
    pub fn set_editor_hover_file(&mut self, path: PathBuf) {
        self.editor_file = Some(path);
        self.hover_only = true;
    }

    pub fn check_module(
        &self,
        module: &Module,
        options: &CompilerOptions,
    ) -> Result<CheckedModule, Vec<Diagnostic>> {
        crate::sema::check_module_with_validation(module, options, self.checks_responsibility())
    }

    pub(crate) fn checks_responsibility(&self) -> bool {
        !self.hover_only
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
        let verified = self.verified_package(path).is_some();
        match path.canonicalize() {
            Ok(canonical) => {
                if verified && canonical != crate::modules_lock::lexical_path(path) {
                    return Err(io::Error::other(
                        "verified SDK paths must not traverse symlinks",
                    ));
                }
                self.resolved_paths
                    .borrow_mut()
                    .entry(path.to_owned())
                    .or_insert_with(|| Some(canonical.clone()));
                if self.track_disk {
                    self.path_kinds
                        .borrow_mut()
                        .entry(canonical.clone())
                        .or_insert_with(|| disk_path_kind(&canonical));
                }
                Ok(canonical)
            }
            Err(original) if verified => Err(original),
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
        if self.verified_package(directory).is_some() {
            return Ok(self
                .verified_directories
                .get(directory)
                .into_iter()
                .flatten()
                .map(|(path, directory)| (path.clone(), *directory))
                .collect());
        }
        let entries = source_entries(directory)?;
        if self.editor_file.is_some() || self.track_disk {
            self.source_directories
                .entry(directory.to_owned())
                .or_insert_with(|| entries.clone());
        }
        Ok(entries)
    }

    pub fn read_bytes(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(bytes) = self.reads.get(path) {
            self.verify_package_bytes(path, bytes)?;
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
        self.verify_package_bytes(path, &bytes)?;
        self.reads.insert(path.to_owned(), bytes.clone());
        Ok(bytes)
    }

    /// Configuration is disk-only, pinned separately from source overlays.
    pub(crate) fn read_config(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let path = self.canonical_path(path)?;
        if let Some(bytes) = self.config_reads.get(&path) {
            self.verify_package_bytes(&path, bytes)?;
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
        self.verify_package_bytes(&path, &bytes)?;
        self.config_reads.insert(path, bytes.clone());
        Ok(bytes)
    }

    pub(crate) fn register_archive(&mut self, root: PathBuf, receipt: ArchiveReceipt) {
        self.register_inventory(root, receipt.files, true);
    }

    pub(crate) fn register_toolchain(&mut self, root: PathBuf, files: BTreeMap<PathBuf, String>) {
        self.register_inventory(root, files, false);
    }

    fn register_inventory(
        &mut self,
        root: PathBuf,
        files: BTreeMap<PathBuf, String>,
        archive: bool,
    ) {
        for relative in files.keys() {
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
                    self.verified_directories
                        .entry(directory.to_owned())
                        .or_default()
                        .insert(child.to_owned(), is_directory);
                }
            }
        }
        self.verified
            .insert(root, VerifiedPackage { files, archive });
    }

    pub(crate) fn read_native_archive(&mut self, root: &Path) -> io::Result<()> {
        let files = &self.verified[root].files;
        crate::modules_lock::check_archive_inventory(root, files)?;
        let inputs: Vec<_> = files
            .keys()
            .filter(|path| path.as_path() != Path::new("rinzu.toml"))
            .map(|path| root.join(path))
            .collect();
        for input in inputs {
            let canonical = self.canonical_path(&input)?;
            self.read_bytes(&canonical)?;
        }
        self.record_native_tree(root);
        Ok(())
    }

    pub(crate) fn is_archive(&self, root: &Path) -> bool {
        self.verified
            .get(root)
            .is_some_and(|package| package.archive)
    }

    fn verified_package(&self, path: &Path) -> Option<(&Path, &VerifiedPackage)> {
        path.ancestors().find_map(|root| {
            self.verified
                .get_key_value(root)
                .map(|(root, package)| (root.as_path(), package))
        })
    }

    fn verify_package_bytes(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some((root, package)) = self.verified_package(path) {
            let expected = package
                .files
                .get(path.strip_prefix(root).expect("verified package ancestor"));
            if !expected.is_some_and(|digest| *digest == crate::standard_library::sha256(bytes)) {
                let remedy = if package.archive {
                    "run `rinzu modules sync`"
                } else {
                    "restore the matching Rinzu toolchain bundle"
                };
                return Err(io::Error::other(format!(
                    "SDK input differs from its verified inventory; {remedy}"
                )));
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
