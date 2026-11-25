//! Sealed frontend artifacts for dependencies selected by the current lock.
use crate::source_package_cache;
use rinzu_compiler::{SdkFrontendCache, SourceSdkInterface, timing};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

const INTERFACE: &str = "interface.msgpack";
const MAX_BYTES: u64 = 64 * 1024 * 1024;

pub(super) struct Pending {
    directory: PathBuf,
    key: String,
    name: String,
    source: Arc<SourceSdkInterface>,
}

impl Pending {
    pub(super) fn store(self) {
        let mut encode = timing::span("frontend.sdk-cache.encode").detail(&self.name);
        let Ok(wire) = rmp_serde::to_vec(&self.source) else {
            return;
        };
        encode.set_counter("bytes", wire.len() as u64);
        if wire.len() as u64 > MAX_BYTES {
            return;
        }
        drop(encode);
        let mut timing = timing::span("frontend.sdk-cache.store").detail(&self.name);
        timing.set_counter("bytes", wire.len() as u64);
        let stored = source_package_cache::store(
            &self.directory,
            &self.key,
            &BTreeMap::from([(INTERFACE.into(), wire)]),
            None,
        )
        .is_ok();
        timing.set_counter("stored", u64::from(stored));
    }
}

pub(crate) struct Cache {
    pub(super) root: PathBuf,
    pub(super) scope: String,
    pub(super) pending: Vec<Pending>,
}

impl Cache {
    fn entry(&self, key: &str) -> Option<(PathBuf, String)> {
        // The scope binds the package key to the executing compiler,
        // project and CLI/toolchain configuration, without application bodies.
        let key = format!("{}:{key}", self.scope);
        source_package_cache::user_directory(&self.root, "jit-sdk-frontend-v1", key.as_bytes()).ok()
    }
}

impl SdkFrontendCache for Cache {
    fn load(&mut self, key: &str, name: &str) -> Option<SourceSdkInterface> {
        let mut timing = timing::span("frontend.sdk-cache.lookup").detail(name);
        let (directory, key) = self.entry(key)?;
        let metadata = directory.join(INTERFACE).symlink_metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return None;
        }
        let files =
            source_package_cache::completed(&directory, &key, &BTreeSet::from([INTERFACE.into()]))?;
        let wire = &files[INTERFACE];
        timing.set_counter("bytes", wire.len() as u64);
        drop(timing);
        let _decode = timing::span("frontend.sdk-cache.decode").detail(name);
        rmp_serde::from_slice(wire).ok()
    }

    fn stage(&mut self, key: &str, name: &str, source: Arc<SourceSdkInterface>) {
        let Some((directory, key)) = self.entry(key) else {
            return;
        };
        self.pending.push(Pending {
            directory,
            key,
            name: name.to_owned(),
            source,
        });
    }
}
