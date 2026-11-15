pub(crate) const POLICY: &str = "MAP_JIT RW -> instruction-cache flush -> RX";
pub(super) const EXPLICIT_ICACHE_SYNC: bool = true;

pub(super) fn map_flags(executable: bool) -> i32 {
    libc::MAP_PRIVATE | libc::MAP_ANON | if executable { libc::MAP_JIT } else { 0 }
}

unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, length: usize);
}

pub(super) unsafe fn sync_instructions(address: *mut libc::c_void, length: usize) {
    // SAFETY: the caller owns freshly written instructions in this live mapping.
    unsafe { sys_icache_invalidate(address, length) };
}
