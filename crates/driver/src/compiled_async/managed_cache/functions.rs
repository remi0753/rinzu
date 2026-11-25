//! Read a sealed pack once and index Cranelift's opaque function stencils.
use super::{AsyncJitCode, Cache, Write};
use rinzu_codegen::AsyncJitFunctionCache;
use rinzu_compiler::timing;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ops::Range;

const PACK: &str = "functions.bin";
type Key = [u8; 32];
const HEADER: usize = 32 + std::mem::size_of::<u32>();

struct Entry {
    bytes: Range<usize>,
    used: Cell<bool>,
}

#[derive(Default)]
pub(super) struct Functions {
    pack: Vec<u8>,
    previous: BTreeMap<Key, Entry>,
    new: BTreeMap<Key, Vec<u8>>,
    new_bytes: usize,
}

impl Functions {
    pub(super) fn load(cache: &Cache) -> Self {
        let mut lookup = timing::span("jit.managed.function-cache.lookup");
        let Some(pack) = cache
            .completed(&[PACK])
            .and_then(|mut files| files.remove(PACK))
        else {
            return Self::default();
        };
        lookup.set_counter("bytes", pack.len() as u64);
        drop(lookup);
        let mut decode = timing::span("jit.managed.function-cache.decode");
        let store = Self::decode(pack).unwrap_or_default();
        decode.set_counter("entries", store.previous.len() as u64);
        store
    }

    fn decode(pack: Vec<u8>) -> Option<Self> {
        let mut previous = BTreeMap::new();
        let mut offset = 0;
        while offset < pack.len() {
            let start = offset + HEADER;
            let (key, size) = pack.get(offset..start)?.split_at(32);
            let size = u32::from_le_bytes(size.try_into().ok()?) as usize;
            let end = start.checked_add(size)?;
            if size == 0 || end > pack.len() {
                return None;
            }
            let entry = Entry {
                bytes: start..end,
                used: Cell::new(false),
            };
            if previous.insert(key.try_into().ok()?, entry).is_some() {
                return None;
            }
            offset = end;
        }
        Some(Self {
            pack,
            previous,
            ..Self::default()
        })
    }

    pub(super) fn capture(&self, cache: Cache) -> Option<Write> {
        if self.new.is_empty() {
            return None;
        }
        let mut timing = timing::span("jit.managed.function-cache.encode");
        let mut pack = Vec::new();
        let mut append = |key: &Key, bytes: &[u8]| {
            if bytes.len() + HEADER > AsyncJitCode::MAX_BYTES - pack.len() {
                return false;
            }
            pack.extend_from_slice(key);
            pack.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            pack.extend_from_slice(bytes);
            true
        };
        let mut retained = 0;
        for (key, bytes) in &self.new {
            retained += u64::from(append(key, bytes));
        }
        for (key, entry) in &self.previous {
            if entry.used.get() && !self.new.contains_key(key) {
                retained += u64::from(append(key, &self.pack[entry.bytes.clone()]));
            }
        }
        timing.set_counter("entries", retained);
        timing.set_counter("bytes", pack.len() as u64);
        Some(Write {
            cache,
            files: BTreeMap::from([(PACK.into(), pack)]),
        })
    }
}

impl AsyncJitFunctionCache for Functions {
    fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> {
        let key: &Key = key.try_into().ok()?;
        let entry = self.previous.get(key)?;
        entry.used.set(true);
        Some(Cow::Borrowed(&self.pack[entry.bytes.clone()]))
    }

    fn insert(&mut self, key: &[u8], value: Vec<u8>) {
        let Ok(key) = <Key>::try_from(key) else {
            return;
        };
        let previous = self.new.get(&key).map_or(0, |bytes| bytes.len() + HEADER);
        let size = self.new_bytes - previous + value.len() + HEADER;
        if !value.is_empty() && size <= AsyncJitCode::MAX_BYTES {
            self.new_bytes = size;
            self.new.insert(key, value);
        }
    }
}
