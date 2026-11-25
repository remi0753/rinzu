//! Restore a checked disk compilation under the same source and resolution evidence.
use crate::{source_native_package, source_package_cache};
use rinzu_bindgen::source_package::sha256;
use rinzu_compiler::{
    CompilationContext, CompilationMode, CompilationOptions, DiskSourceEvidence,
    FunctionFrontendCache, NativeCliInputs, timing,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

mod sdk;
pub(crate) use sdk::Cache as SdkCache;

const CHECKED: &str = "checked.msgpack";
const FUNCTIONS: &str = "functions.msgpack";
// An application context includes the checked program and all its resolved
// source interfaces. Keep it below the source cache's 256 MiB namespace limit.
const MAX_BYTES: u64 = 192 * 1024 * 1024;

pub(crate) struct Cache {
    directory: PathBuf,
    key: String,
    mode: CompilationMode,
    manifest_path: Option<PathBuf>,
    canonical_input: PathBuf,
    cwd: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved<C, E> {
    context: C,
    sources: E,
}

pub(crate) struct Pending {
    cache: Cache,
    wire: Option<Vec<u8>>,
    sdks: Vec<sdk::Pending>,
    functions: Option<FunctionFrontendCache>,
    sources: DiskSourceEvidence,
}

impl Cache {
    pub(crate) fn open(
        input: &Path,
        options: &CompilationOptions,
        native: &NativeCliInputs,
    ) -> Option<Self> {
        let _timing = timing::span("jit.frontend.cache.key");
        let canonical_input = input.canonicalize().ok()?;
        let kind = canonical_input.metadata().ok()?;
        let (mode, manifest_path) = if kind.is_dir() {
            let path = rinzu_compiler::find_manifest(&canonical_input).ok()?;
            (CompilationMode::Package, Some(path))
        } else if kind.is_file() {
            (CompilationMode::File, None)
        } else {
            return None;
        };
        let cwd = std::env::current_dir().ok()?;
        let resources = rinzu_compiler::standard_library::resource_root().ok()?;
        let wire = serde_json::to_vec(&(
            "rinzu.checked-run-v1",
            &canonical_input,
            &cwd,
            mode,
            &manifest_path,
            options,
            native,
            resources,
            source_native_package::compiler_digest().ok()?,
        ))
        .ok()?;
        let (directory, key) =
            source_package_cache::user_directory(&canonical_input, "jit-frontend-v1", &wire)
                .ok()?;
        Some(Self {
            directory,
            key,
            mode,
            manifest_path,
            canonical_input,
            cwd,
        })
    }

    fn scope_is_current(&self, context: &CompilationContext) -> bool {
        context.mode == self.mode
            && context.cwd == self.cwd
            && if let Some(manifest_path) = &self.manifest_path {
                manifest_path == &context.metadata.root.join("rinzu.toml")
                    && rinzu_compiler::find_manifest(&self.canonical_input)
                        .is_ok_and(|current| &current == manifest_path)
            } else {
                context.entry == self.canonical_input
            }
    }

    pub(crate) fn sdk_cache(&self) -> Option<SdkCache> {
        (self.mode == CompilationMode::Package).then(|| SdkCache {
            root: self.canonical_input.clone(),
            scope: self.key.clone(),
            pending: Vec::new(),
        })
    }

    fn function_cache_entry(&self) -> Option<(PathBuf, String)> {
        source_package_cache::user_directory(
            &self.canonical_input,
            "jit-function-frontend-v1",
            self.key.as_bytes(),
        )
        .ok()
    }

    pub(crate) fn function_cache(&self) -> Option<FunctionFrontendCache> {
        (self.mode == CompilationMode::Package)
            .then(|| self.restore_functions().unwrap_or_default())
    }

    fn restore_functions(&self) -> Option<FunctionFrontendCache> {
        let mut lookup = timing::span("frontend.function-cache.lookup");
        let (directory, key) = self.function_cache_entry()?;
        let metadata = directory.join(FUNCTIONS).symlink_metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return None;
        }
        let files =
            source_package_cache::completed(&directory, &key, &BTreeSet::from([FUNCTIONS.into()]))?;
        let wire = &files[FUNCTIONS];
        lookup.set_counter("bytes", wire.len() as u64);
        drop(lookup);
        let _decode = timing::span("frontend.function-cache.decode");
        rmp_serde::from_slice(wire).ok()
    }

    pub(crate) fn restore(
        &self,
        options: &CompilationOptions,
        native: &NativeCliInputs,
    ) -> Option<(CompilationContext, String)> {
        let result = self.read(options, native);
        let _timing = timing::span(if result.is_some() {
            "jit.frontend.cache.hit"
        } else {
            "jit.frontend.cache.miss"
        });
        result
    }

    fn read(
        &self,
        options: &CompilationOptions,
        native: &NativeCliInputs,
    ) -> Option<(CompilationContext, String)> {
        let mut timing = timing::span("jit.frontend.cache.lookup");
        let metadata = self.directory.join(CHECKED).symlink_metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return None;
        }
        let files = source_package_cache::completed(
            &self.directory,
            &self.key,
            &BTreeSet::from([CHECKED.into()]),
        )?;
        let wire = &files[CHECKED];
        timing.set_counter("bytes", wire.len() as u64);
        drop(timing);
        let mut timing = timing::span("jit.frontend.cache.decode");
        let Saved { context, sources }: Saved<CompilationContext, DiskSourceEvidence> =
            rmp_serde::from_slice(wire).ok()?;
        timing.set_counter("functions", context.artifact.ir.functions.len() as u64);
        drop(timing);
        let mut timing = timing::span("jit.frontend.cache.verify_inputs");
        crate::jit_progress::phase("Validating cached inputs");
        timing.set_counter("files", context.artifact.checked_inputs.len() as u64);
        if !self.scope_is_current(&context)
            || context.target != options.target
            || context.options.language != options.language
            || context.options.target != options.target
            || context.artifact.is_file_input != (self.mode == CompilationMode::File)
            || context.entry.canonicalize().ok()? != context.entry
            || !sources.disk_is_current(&context.artifact.checked_inputs)
        {
            return None;
        }
        let mut cli_owner = context.metadata.clone();
        if self.mode == CompilationMode::File {
            cli_owner.root.clone_from(&context.cwd);
        }
        if native.normalize(&cli_owner).ok()? != context.artifact.native_inputs {
            return None;
        }
        drop(timing);
        let _verify = timing::span("jit.frontend.cache.verify_ir");
        crate::jit_progress::phase("Verifying cached IR");
        rinzu_compiler::verify_module(&context.artifact.ir).ok()?;
        Some((context, self.generation(wire)))
    }

    pub(crate) fn prepare(
        self,
        context: &CompilationContext,
        sources: DiskSourceEvidence,
        sdk_cache: Option<SdkCache>,
        functions: Option<FunctionFrontendCache>,
    ) -> Option<(Pending, Option<String>)> {
        let mut timing = timing::span("jit.frontend.cache.encode");
        if !self.scope_is_current(context) {
            return None;
        }
        crate::jit_progress::phase("Preparing frontend cache");
        let wire = rmp_serde::to_vec(&Saved {
            context,
            sources: &sources,
        })
        .ok()
        .filter(|wire| {
            timing.set_counter("bytes", wire.len() as u64);
            wire.len() as u64 <= MAX_BYTES
        });
        let sdks = sdk_cache.map_or_else(Vec::new, |cache| cache.pending);
        if wire.is_none() && sdks.is_empty() && functions.is_none() {
            return None;
        }
        let generation = wire.as_ref().map(|bytes| self.generation(bytes));
        Some((
            Pending {
                cache: self,
                wire,
                sdks,
                functions,
                sources,
            },
            generation,
        ))
    }

    fn store_functions(&self, functions: FunctionFrontendCache) {
        let mut encode = timing::span("frontend.function-cache.encode");
        let Ok(wire) = rmp_serde::to_vec(&functions) else {
            return;
        };
        encode.set_counter("bytes", wire.len() as u64);
        if wire.len() as u64 > MAX_BYTES {
            return;
        }
        drop(encode);
        let mut store = timing::span("frontend.function-cache.store");
        let Some((directory, key)) = self.function_cache_entry() else {
            return;
        };
        let stored = source_package_cache::store(
            &directory,
            &key,
            &BTreeMap::from([(FUNCTIONS.into(), wire)]),
            None,
        )
        .is_ok();
        store.set_counter("stored", u64::from(stored));
    }

    fn generation(&self, wire: &[u8]) -> String {
        // Downstream machine and runtime code depend on the compiler and
        // scope even when the serialized checked program is identical.
        sha256(format!("{}:{}", self.key, sha256(wire)).as_bytes())
    }
}

impl Pending {
    pub(crate) fn store(self, context: &CompilationContext) {
        if !self.cache.scope_is_current(context)
            || !self
                .sources
                .disk_is_current(&context.artifact.checked_inputs)
        {
            return;
        }
        if let Some(wire) = self.wire {
            let mut timing = timing::span("jit.frontend.cache.store");
            timing.set_counter("bytes", wire.len() as u64);
            let stored = source_package_cache::store(
                &self.cache.directory,
                &self.cache.key,
                &BTreeMap::from([(CHECKED.into(), wire)]),
                None,
            )
            .is_ok();
            timing.set_counter("stored", u64::from(stored));
        }
        if let Some(functions) = self.functions {
            self.cache.store_functions(functions);
        }
        for sdk in self.sdks {
            sdk.store();
        }
    }
}
