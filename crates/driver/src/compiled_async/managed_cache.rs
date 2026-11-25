//! Persist a successful checked program's relocatable Cranelift definitions.
use super::*;
use crate::source_package_cache;
use rinzu_codegen::{AsyncJitCode, AsyncJitCompiler, AsyncJitFunctionCache, AsyncJitImage};
use rinzu_compiler::timing;
use std::collections::BTreeSet;
use std::fs::{Metadata, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

mod functions;

const IMAGE: &str = "code.bin";
const METADATA: &str = "metadata.msgpack";

struct Cache {
    directory: PathBuf,
    key: String,
    uid: u32,
}

pub(super) struct Pending {
    image: Option<Write>,
    functions: Option<Write>,
}

struct Write {
    cache: Cache,
    files: BTreeMap<String, Vec<u8>>,
}

pub(super) fn prepare(
    manifest: &Manifest,
    artifact: &FrontendArtifact,
    aot: &BaselineAotObject,
    imports: &HashMap<String, u64>,
    symbols: &[String],
    generation: Option<&str>,
) -> Result<(PreparedAsyncJit, Option<Pending>), String> {
    let compiler = AsyncJitCompiler::new().map_err(|error| format!("{error:#}"))?;
    let cache =
        generation.and_then(|generation| Cache::image(&manifest.root, &compiler, generation));
    if let Some(cache) = &cache {
        let result = cache.restore(&compiler, aot, symbols);
        let _timing = timing::span(if result.is_some() {
            "jit.managed.cache.hit"
        } else {
            "jit.managed.cache.miss"
        });
        if let Some(prepared) = result {
            return Ok((prepared, None));
        }
    }
    let function_cache = cache
        .as_ref()
        .and_then(|_| Cache::functions(&manifest.root, &compiler));
    let mut functions = function_cache.as_ref().map(functions::Functions::load);
    let (prepared, image) = compiler
        .prepare_with_cache(
            &artifact.ir,
            imports,
            cache.is_some(),
            functions
                .as_mut()
                .map(|cache| cache as &mut dyn AsyncJitFunctionCache),
        )
        .map_err(|error| format!("{error:#}"))?;
    let image = cache
        .zip(image)
        .and_then(|(cache, image)| cache.capture(image));
    let functions = function_cache
        .zip(functions)
        .and_then(|(cache, functions)| functions.capture(cache));
    let pending = (image.is_some() || functions.is_some()).then_some(Pending { image, functions });
    Ok((prepared, pending))
}

impl Cache {
    fn image(root: &Path, compiler: &AsyncJitCompiler, generation: &str) -> Option<Self> {
        let _timing = timing::span("jit.managed.cache.key");
        let wire = serde_json::to_vec(&(
            generation,
            rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION,
            compiler.cache_identity(),
        ))
        .ok()?;
        Self::open(root, "jit-code-v1", &wire)
    }

    fn functions(root: &Path, compiler: &AsyncJitCompiler) -> Option<Self> {
        let _timing = timing::span("jit.managed.function-cache.key");
        let wire = serde_json::to_vec(&(
            crate::source_native_package::compiler_digest().ok()?,
            rinzu_codegen::BASELINE_RUNTIME_ABI_REVISION,
            compiler.cache_identity(),
        ))
        .ok()?;
        Self::open(root, "jit-functions-v1", &wire)
    }

    fn open(root: &Path, namespace: &str, wire: &[u8]) -> Option<Self> {
        let (directory, key) = source_package_cache::user_directory(root, namespace, wire).ok()?;
        // SAFETY: geteuid reads the process's effective user ID.
        let uid = unsafe { libc::geteuid() };
        let cache = Self {
            directory,
            key,
            uid,
        };
        cache.owned_directories().then_some(cache)
    }

    fn owned_directories(&self) -> bool {
        // Check the entry, its namespace and the configured user cache root.
        self.directory.ancestors().take(3).all(|path| {
            path.symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir() && owned(&metadata, self.uid))
        })
    }

    fn restore(
        &self,
        compiler: &AsyncJitCompiler,
        aot: &BaselineAotObject,
        symbols: &[String],
    ) -> Option<PreparedAsyncJit> {
        let mut timing = timing::span("jit.managed.cache.lookup");
        let files = self.completed(&[IMAGE, METADATA])?;
        timing.set_counter(
            "bytes",
            files.values().map(|bytes| bytes.len() as u64).sum(),
        );
        drop(timing);
        let timing = timing::span("jit.managed.cache.decode");
        let image = rmp_serde::from_slice(&files[METADATA]).ok()?;
        drop(timing);
        compiler.restore(aot, symbols, image, &files[IMAGE]).ok()
    }

    fn completed(&self, names: &[&str]) -> Option<BTreeMap<String, Vec<u8>>> {
        let names = names
            .iter()
            .map(|name| (*name).into())
            .collect::<BTreeSet<_>>();
        source_package_cache::completed_with_reader(
            &self.directory,
            &self.key,
            &[],
            |_| Some((names, ())),
            |path| self.read(path),
        )
        .map(|(files, ())| files)
    }

    fn read(&self, path: &Path) -> Option<Vec<u8>> {
        let limit = if path.file_name()?.to_str()? == "receipt.json" {
            4096
        } else {
            AsyncJitCode::MAX_BYTES as u64
        };
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .ok()?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || !owned(&metadata, self.uid) || metadata.len() > limit {
            return None;
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(limit + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() as u64 <= limit).then_some(bytes)
    }

    fn capture(self, image: AsyncJitImage) -> Option<Write> {
        let _timing = timing::span("jit.managed.cache.encode");
        let metadata = rmp_serde::to_vec(&image.metadata).ok()?;
        (metadata.len() <= AsyncJitCode::MAX_BYTES).then_some(Write {
            cache: self,
            files: BTreeMap::from([(IMAGE.into(), image.bytes), (METADATA.into(), metadata)]),
        })
    }
}

impl Pending {
    pub(super) fn store(self) {
        if let Some(image) = self.image {
            image.store("jit.managed.cache.store");
        }
        if let Some(functions) = self.functions {
            functions.store("jit.managed.function-cache.store");
        }
    }
}

impl Write {
    fn store(self, phase: &'static str) {
        let mut timing = timing::span(phase);
        if self.cache.owned_directories() {
            timing.set_counter(
                "bytes",
                self.files.values().map(|bytes| bytes.len() as u64).sum(),
            );
            let stored = source_package_cache::store(
                &self.cache.directory,
                &self.cache.key,
                &self.files,
                None,
            );
            timing.set_counter("stored", u64::from(stored.is_ok()));
        }
    }
}

fn owned(metadata: &Metadata, uid: u32) -> bool {
    metadata.uid() == uid && metadata.mode() & 0o022 == 0
}
