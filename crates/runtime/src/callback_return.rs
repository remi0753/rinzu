//! Immutable native copies retained until OS process termination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ReturnBufferStats {
    pub normal: usize,
    pub fallback: usize,
    pub bytes: usize,
    pub metadata_bytes: usize,
    pub normal_bytes: usize,
    pub fallback_bytes: usize,
}
unsafe extern "C" {
    fn rinzu_callback_return_copy(
        input: *const u8,
        prefix: usize,
        length: usize,
        string: u32,
        fallback: u32,
    ) -> *const u8;
    fn rinzu_callback_return_stats() -> ReturnBufferStats;
    fn rinzu_callback_return_node_size() -> usize;
    fn rinzu_callback_return_copy_bytes() -> usize;
    fn rinzu_callback_scope_prepare() -> u64;
    fn rinzu_callback_scope_enter(id: u64) -> u32;
    fn rinzu_callback_scope_finish(id: u64, mode: u32) -> u32;
    fn rinzu_callback_scope_copy(
        id: u64,
        input: *const u8,
        prefix: usize,
        length: usize,
        string: u32,
        fallback: u32,
    ) -> *const u8;
    fn rinzu_callback_scope_stats() -> ScopeStats;
    fn rinzu_callback_return_snapshot() -> ReturnSnapshot;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ReturnSnapshot {
    pub process: ReturnBufferStats,
    pub scopes: ScopeStats,
    pub process_copied_bytes: usize,
}
/// One arena-lock snapshot, including promotion transfers between both owners.
#[must_use]
pub fn snapshot() -> ReturnSnapshot {
    // SAFETY: the C helper copies counters under their common arena mutex.
    unsafe { rinzu_callback_return_snapshot() }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ScopeStats {
    pub allocated: usize,
    pub released: usize,
    pub promoted: usize,
    pub allocated_bytes: usize,
    pub released_bytes: usize,
    pub promoted_bytes: usize,
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub copied_bytes: usize,
    pub active_scopes: usize,
    pub metadata_bytes: usize,
}

/// Actual allocation owner, distinct from callback admission and GC roots.
#[derive(Debug)]
pub struct ReturnScope {
    id: u64,
}
impl Default for ReturnScope {
    fn default() -> Self {
        Self::new()
    }
}
impl ReturnScope {
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: the private C registry creates a unique non-pointer generation.
        let id = unsafe { rinzu_callback_scope_prepare() };
        Self { id }
    }
    #[must_use]
    pub fn enter(&self) -> bool {
        // SAFETY: lookup validates the ID/state without dereferencing stale addresses.
        unsafe { rinzu_callback_scope_enter(self.id) != 0 }
    }
    /// # Safety
    /// Every future use is closed by the exact consumer proof, the physical native
    /// call returned, and all payload-capable factories, callback admissions,
    /// receipts and children settled. Managed-only children own no native copy.
    pub unsafe fn consume(&self) -> bool {
        // SAFETY: caller supplies the complete static and dynamic closure contract.
        unsafe { rinzu_callback_scope_finish(self.id, 1) != 0 }
    }
    /// # Safety
    /// No subsequent factory/entry uses this owner. Existing exported pointers
    /// may still be read; promotion retains their actual allocations permanently.
    pub unsafe fn promote(&self) -> bool {
        // SAFETY: caller stops new factories; C transfers the actual owner list.
        unsafe { rinzu_callback_scope_finish(self.id, 2) != 0 }
    }
    fn copy(
        self: &std::sync::Arc<Self>,
        prefix: &[u8],
        length: usize,
        string: bool,
        fallback: bool,
    ) -> ScopedBuffer {
        // SAFETY: prefix is live through the synchronous copy. The registry owns
        // the copy independently; no slice into it is exposed by ScopedBuffer.
        let address = unsafe {
            rinzu_callback_scope_copy(
                self.id,
                prefix.as_ptr(),
                prefix.len(),
                length,
                u32::from(string),
                u32::from(fallback),
            )
        } as usize;
        ScopedBuffer {
            address,
            bytes: length
                .checked_add(usize::from(string))
                .unwrap_or_else(|| fatal_size())
                .max(1),
            _owner: std::sync::Arc::clone(self),
        }
    }
}
impl Drop for ReturnScope {
    fn drop(&mut self) {
        // SAFETY: no safe borrowed contents exist. Prepared is unpublished;
        // entered without a receipt is promoted. Retired IDs are harmless.
        unsafe {
            rinzu_callback_scope_finish(self.id, 3);
        }
    }
}
/// No Copy and no safe/static slice API. Raw-pointer use obeys native scope rules.
/// ```compile_fail
/// fn duplicate(buffer: rinzu_runtime::callback_return::ScopedBuffer) {
///     let first = buffer;
///     let second = buffer;
/// }
/// ```
#[derive(Debug)]
pub struct ScopedBuffer {
    address: usize,
    bytes: usize,
    _owner: std::sync::Arc<ReturnScope>,
}
#[derive(Debug)]
pub enum ProducedBuffer {
    Process(ReturnBuffer),
    Scoped(ScopedBuffer),
}
impl ProducedBuffer {
    #[must_use]
    pub fn string(text: &str, fallback: bool, owner: Option<&std::sync::Arc<ReturnScope>>) -> Self {
        match owner {
            Some(owner) => Self::Scoped(owner.copy(text.as_bytes(), text.len(), true, fallback)),
            None => Self::Process(ReturnBuffer::string(text, fallback)),
        }
    }
    #[must_use]
    pub fn bytes(
        prefix: &[u8],
        length: u64,
        fallback: bool,
        owner: Option<&std::sync::Arc<ReturnScope>>,
    ) -> Self {
        let length = usize::try_from(length).unwrap_or_else(|_| fatal_size());
        match owner {
            Some(owner) => Self::Scoped(owner.copy(prefix, length, false, fallback)),
            None => Self::Process(ReturnBuffer::bytes(prefix, length as u64, fallback)),
        }
    }
    #[must_use]
    pub fn bits(&self) -> u64 {
        match self {
            Self::Process(buffer) => buffer.bits(),
            Self::Scoped(buffer) => buffer.address as u64,
        }
    }
    #[must_use]
    pub fn storage_bytes(&self) -> usize {
        match self {
            Self::Process(buffer) => buffer.storage_bytes(),
            Self::Scoped(buffer) => buffer.bytes,
        }
    }
    #[must_use]
    pub fn metadata_bytes(&self) -> usize {
        // SAFETY: same-build layout helper, independent of any live allocation.
        unsafe { rinzu_callback_return_node_size() }
    }
    #[must_use]
    pub const fn is_scoped(&self) -> bool {
        matches!(self, Self::Scoped(_))
    }
}
#[must_use]
pub fn scope_stats() -> ScopeStats {
    // SAFETY: C snapshots its initialized counters under the arena mutex.
    unsafe { rinzu_callback_scope_stats() }
}

#[derive(Clone, Copy, Debug)]
pub struct ReturnBuffer {
    address: usize,
    bytes: usize,
}
impl ReturnBuffer {
    #[must_use]
    pub fn string(text: &str, fallback: bool) -> Self {
        Self::copy(text.as_bytes(), text.len(), true, fallback)
    }
    #[must_use]
    pub fn bytes(prefix: &[u8], length: u64, fallback: bool) -> Self {
        let length = usize::try_from(length).unwrap_or_else(|_| fatal_size());
        Self::copy(prefix, length, false, fallback)
    }
    fn copy(prefix: &[u8], length: usize, string: bool, fallback: bool) -> Self {
        // SAFETY: prefix is an immutable live slice; the C arena checks widths,
        // copies it before returning and never frees or mutates the new buffer.
        let pointer = unsafe {
            rinzu_callback_return_copy(
                prefix.as_ptr(),
                prefix.len(),
                length,
                u32::from(string),
                u32::from(fallback),
            )
        };
        Self {
            address: pointer as usize,
            bytes: length
                .checked_add(usize::from(string))
                .unwrap_or_else(|| fatal_size())
                .max(1),
        }
    }
    #[must_use]
    pub fn as_ptr(self) -> *const u8 {
        self.address as *const u8
    }
    #[must_use]
    pub fn bits(self) -> u64 {
        self.address as u64
    }
    #[must_use]
    pub fn storage_bytes(self) -> usize {
        self.bytes
    }
    #[must_use]
    pub fn metadata_bytes(self) -> usize {
        // SAFETY: private same-build C helper returns the actual arena node layout size.
        unsafe { rinzu_callback_return_node_size() }
    }
    #[must_use]
    pub fn contents(self) -> &'static [u8] {
        // SAFETY: the non-NULL initialized allocation is immutable and held for
        // process lifetime, independently of managed heaps/Owners and this value.
        unsafe { std::slice::from_raw_parts(self.as_ptr(), self.bytes) }
    }
}
fn fatal_size() -> ! {
    // SAFETY: known target-size/allocation exhaustion terminates without cleanup.
    unsafe { libc::_exit(71) }
}
#[must_use]
pub fn stats() -> ReturnBufferStats {
    // SAFETY: C takes its initialized process mutex and returns a value snapshot.
    unsafe { rinzu_callback_return_stats() }
}
/// Cumulative explicit memcpy prefix in the process factory; promotion performs
/// no new copy and is counted only by the original scoped factory.
#[must_use]
pub fn process_copy_bytes() -> usize {
    // SAFETY: a locked immutable counter snapshot, without native pointers.
    unsafe { rinzu_callback_return_copy_bytes() }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn scoped_generation_once_padding_and_missing_receipt_preserve_native_contract() {
        let before = scope_stats();
        let scope = Arc::new(ReturnScope::new());
        let buffer = ProducedBuffer::string("A\0B", false, Some(&scope));
        let fallback = ProducedBuffer::bytes(&[42], 4, true, Some(&scope));
        let zero = ProducedBuffer::bytes(&[], 0, false, Some(&scope));
        assert!(buffer.is_scoped() && scope.enter() && !scope.enter());
        // SAFETY: copies remain owned by scope and no consumption has occurred.
        unsafe {
            assert_eq!(
                std::slice::from_raw_parts(buffer.bits() as *const u8, 4),
                b"A\0B\0"
            );
            assert_eq!(
                std::slice::from_raw_parts(fallback.bits() as *const u8, 4),
                &[42, 0, 0, 0]
            );
            assert_eq!(*(zero.bits() as *const u8), 0);
            // This fixture is the only consumer and all its physical reads ended.
            assert!(scope.consume());
            assert!(!scope.consume());
        }
        drop((buffer, fallback, zero, scope));
        let missing = Arc::new(ReturnScope::new());
        let buffer = ProducedBuffer::bytes(&[42], 1, false, Some(&missing));
        let address = buffer.bits() as *const u8;
        assert!(missing.enter());
        drop((buffer, missing));
        // SAFETY: the entered guard had no consumption receipt, so C promoted
        // its actual buffer into the immutable process arena before retiring it.
        assert_eq!(unsafe { *address }, 42);
        let after = scope_stats();
        assert_eq!(after.released - before.released, 3);
        assert_eq!(after.promoted - before.promoted, 1);
        assert_eq!(after.live_bytes, before.live_bytes);
        assert_eq!(after.active_scopes, before.active_scopes);
    }
}
