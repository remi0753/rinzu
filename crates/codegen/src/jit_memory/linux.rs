pub(crate) const POLICY: &str = "anonymous mmap RW -> RX";
pub(super) const EXPLICIT_ICACHE_SYNC: bool = false;

pub(super) fn map_flags(_: bool) -> i32 {
    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS
}

// x86 instruction and data caches are coherent. Publication precedes all calls
// to the new code, including calls made by a Worker.
pub(super) unsafe fn sync_instructions(_: *mut libc::c_void, _: usize) {}
