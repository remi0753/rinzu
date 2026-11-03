use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use cranelift_jit::{BranchProtection, JITMemoryKind, JITMemoryProvider};
use cranelift_module::{ModuleError, ModuleResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Protection {
    Executable,
    ReadOnly,
    Writable,
}

#[derive(Debug)]
struct Allocation {
    address: *mut libc::c_void,
    length: usize,
    protection: Protection,
    finalized: bool,
    used: usize,
}

// SAFETY: allocations are process mappings. Access and protection changes are
// serialized through the JIT module, which owns this provider.
unsafe impl Send for Allocation {}

pub(crate) struct MacOsArm64JitMemory {
    page_size: usize,
    allocations: Vec<Allocation>,
    readonly_current: Option<usize>,
    audit: Arc<JitMemoryAudit>,
}

#[derive(Debug, Default)]
pub(crate) struct JitMemoryAudit {
    executable_mappings: AtomicUsize,
    executable_bytes: AtomicUsize,
    icache_flushes: AtomicUsize,
    rx_transitions: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct JitMemoryAuditSnapshot {
    pub executable_mappings: usize,
    pub executable_bytes: usize,
    pub icache_flushes: usize,
    pub rx_transitions: usize,
    pub rwx_mappings: usize,
}

impl JitMemoryAudit {
    pub(crate) fn snapshot(&self) -> JitMemoryAuditSnapshot {
        JitMemoryAuditSnapshot {
            executable_mappings: self.executable_mappings.load(Ordering::Acquire),
            executable_bytes: self.executable_bytes.load(Ordering::Acquire),
            icache_flushes: self.icache_flushes.load(Ordering::Acquire),
            rx_transitions: self.rx_transitions.load(Ordering::Acquire),
            rwx_mappings: 0,
        }
    }
}

impl MacOsArm64JitMemory {
    pub(crate) fn new_with_audit() -> Result<(Self, Arc<JitMemoryAudit>)> {
        // SAFETY: sysconf reads an immutable process-wide kernel setting.
        let page_size = query_page_size();
        if page_size <= 0 {
            return rinzu_runtime::allocation::checked_io(Err(io::Error::last_os_error()))
                .context("query page size");
        }
        let audit = Arc::new(JitMemoryAudit::default());
        Ok((
            Self {
                page_size: usize::try_from(page_size).expect("positive page size"),
                allocations: Vec::new(),
                readonly_current: None,
                audit: Arc::clone(&audit),
            },
            audit,
        ))
    }

    fn rounded_length(&self, size: usize) -> usize {
        let size = size.max(1);
        size.checked_add(self.page_size - 1)
            .map(|value| value / self.page_size * self.page_size)
            .unwrap_or_else(|| rinzu_runtime::allocation::fatal())
    }

    fn readonly_slot(&mut self, size: usize, align: usize) -> Option<*mut u8> {
        let allocation = self.allocations.get_mut(self.readonly_current?)?;
        if allocation.finalized || allocation.protection != Protection::ReadOnly {
            return None;
        }
        let offset = allocation.used.checked_add(align - 1)? & !(align - 1);
        let end = offset.checked_add(size.max(1))?;
        if end > allocation.length {
            return None;
        }
        allocation.used = end;
        // SAFETY: offset..end is a distinct nonempty range in this live writable
        // mapping; the page base and offset satisfy the requested alignment.
        Some(unsafe { allocation.address.cast::<u8>().add(offset) })
    }

    fn protect(allocation: &mut Allocation, protection: i32) -> io::Result<()> {
        let result = protect_pages(allocation, protection);
        if result != 0 {
            return rinzu_runtime::allocation::checked_io(Err(io::Error::last_os_error()));
        }
        allocation.finalized = true;
        Ok(())
    }
}

impl JITMemoryProvider for MacOsArm64JitMemory {
    fn allocate(&mut self, size: usize, align: u64, kind: JITMemoryKind) -> io::Result<*mut u8> {
        let align = usize::try_from(align)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "alignment overflow"))?
            .max(1);
        if align > self.page_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "alignment exceeds the JIT page size",
            ));
        }
        let protection = match kind {
            JITMemoryKind::Executable => Protection::Executable,
            JITMemoryKind::ReadOnly => Protection::ReadOnly,
            JITMemoryKind::Writable => Protection::Writable,
        };
        if protection == Protection::ReadOnly
            && align.is_power_of_two()
            && let Some(pointer) = self.readonly_slot(size, align)
        {
            return Ok(pointer);
        }
        let length = self.rounded_length(size);
        let flags = libc::MAP_PRIVATE
            | libc::MAP_ANON
            | if protection == Protection::Executable {
                libc::MAP_JIT
            } else {
                0
            };
        // No mapping is ever writable and executable at the same time.
        let address = map_pages(length, flags);
        if address == libc::MAP_FAILED {
            return rinzu_runtime::allocation::checked_io(Err(io::Error::last_os_error()));
        }
        self.allocations.push(Allocation {
            address,
            length,
            protection,
            finalized: protection == Protection::Writable,
            used: size.max(1),
        });
        if protection == Protection::ReadOnly && size.max(1) < self.page_size {
            self.readonly_current = Some(self.allocations.len() - 1);
        }
        if protection == Protection::Executable {
            self.audit
                .executable_mappings
                .fetch_add(1, Ordering::Relaxed);
            self.audit
                .executable_bytes
                .fetch_add(length, Ordering::Relaxed);
        }
        Ok(address.cast())
    }

    unsafe fn free_memory(&mut self) {
        self.readonly_current = None;
        for allocation in self.allocations.drain(..) {
            // SAFETY: every allocation is a live mapping created exactly once above.
            unsafe { libc::munmap(allocation.address, allocation.length) };
        }
    }

    fn finalize(&mut self, branch_protection: BranchProtection) -> ModuleResult<()> {
        if branch_protection != BranchProtection::None {
            return Err(ModuleError::Backend(anyhow::anyhow!(
                "arm64 branch protection is not enabled by the I0 backend spike"
            )));
        }
        for allocation in &mut self.allocations {
            if allocation.finalized {
                continue;
            }
            match allocation.protection {
                Protection::Executable => {
                    // SAFETY: this mapping contains freshly written instructions.
                    unsafe {
                        sys_icache_invalidate(allocation.address, allocation.length);
                    }
                    self.audit.icache_flushes.fetch_add(1, Ordering::Relaxed);
                    Self::protect(allocation, libc::PROT_READ | libc::PROT_EXEC).map_err(
                        |error| {
                            ModuleError::Backend(
                                anyhow::Error::new(error)
                                    .context("make JIT code readable-executable"),
                            )
                        },
                    )?;
                    self.audit.rx_transitions.fetch_add(1, Ordering::Release);
                }
                Protection::ReadOnly => {
                    Self::protect(allocation, libc::PROT_READ).map_err(|error| {
                        ModuleError::Backend(
                            anyhow::Error::new(error).context("make JIT data read-only"),
                        )
                    })?;
                }
                Protection::Writable => allocation.finalized = true,
            }
        }
        self.readonly_current = None;
        Ok(())
    }
}

impl Drop for MacOsArm64JitMemory {
    fn drop(&mut self) {
        // SAFETY: dropping the owning JIT module invalidates all of its code pointers.
        unsafe { self.free_memory() };
    }
}

unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, length: usize);
}

fn query_page_size() -> libc::c_long {
    #[cfg(test)]
    if tests::inject_failure(tests::Operation::PageSize) {
        return -1;
    }
    // SAFETY: sysconf reads an immutable process-wide kernel setting.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) }
}

fn map_pages(length: usize, flags: i32) -> *mut libc::c_void {
    #[cfg(test)]
    if tests::inject_failure(tests::Operation::Map) {
        return libc::MAP_FAILED;
    }
    // SAFETY: request a new anonymous whole-page mapping with no file backing.
    unsafe {
        libc::mmap(
            core::ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            flags,
            -1,
            0,
        )
    }
}

fn protect_pages(allocation: &Allocation, protection: i32) -> i32 {
    #[cfg(test)]
    if tests::inject_failure(tests::Operation::Protect) {
        return -1;
    }
    // SAFETY: allocation identifies a live whole-page mmap owned by the provider.
    unsafe { libc::mprotect(allocation.address, allocation.length, protection) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy, Eq, PartialEq)]
    pub(super) enum Operation {
        PageSize,
        Map,
        Protect,
    }
    thread_local! {
        static FAILURE: Cell<Option<(Operation, i32)>> = const { Cell::new(None) };
    }
    pub(super) fn inject_failure(operation: Operation) -> bool {
        FAILURE.with(|failure| {
            if let Some((expected, code)) = failure.get()
                && operation == expected
            {
                failure.set(None);
                // SAFETY: __error returns this thread's errno location on Darwin.
                unsafe { *libc::__error() = code };
                true
            } else {
                false
            }
        })
    }

    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            panic!("mapping OOM must not unwind or clean up");
        }
    }

    #[test]
    fn allocation_failure_child() {
        let Ok(mode) = std::env::var("RINZU_TEST_JIT_MEMORY_FAILURE") else {
            return;
        };
        let _cleanup = Cleanup;
        if mode == "page" {
            FAILURE.with(|failure| failure.set(Some((Operation::PageSize, libc::ENOMEM))));
            let _ = MacOsArm64JitMemory::new_with_audit();
        } else {
            let (mut memory, _) = MacOsArm64JitMemory::new_with_audit().unwrap();
            match mode.as_str() {
                "size" => {
                    let _ = memory.allocate(usize::MAX, 8, JITMemoryKind::Writable);
                }
                "map" => {
                    FAILURE.with(|failure| failure.set(Some((Operation::Map, libc::ENOMEM))));
                    let _ = memory.allocate(64, 8, JITMemoryKind::Writable);
                }
                "protect" => {
                    memory.allocate(64, 8, JITMemoryKind::ReadOnly).unwrap();
                    FAILURE.with(|failure| failure.set(Some((Operation::Protect, libc::ENOMEM))));
                    let _ = memory.finalize(BranchProtection::None);
                }
                _ => panic!("unknown failure operation"),
            }
        }
        panic!("memory failure returned to the JIT caller");
    }

    #[test]
    fn known_memory_failures_exit_before_error_context_or_cleanup() {
        for mode in ["size", "page", "map", "protect"] {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "macos_jit_memory::tests::allocation_failure_child",
                    "--nocapture",
                ])
                .env("RINZU_TEST_JIT_MEMORY_FAILURE", mode)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let started = Instant::now();
            loop {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                if started.elapsed() > Duration::from_secs(10) {
                    child.kill().unwrap();
                    panic!("{mode}: fatal memory failure did not terminate");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.code(), Some(71), "{mode}: {output:?}");
            assert_eq!(
                output.stderr, b"rinzu: required runtime resource unavailable (71)\n",
                "{mode}: {output:?}"
            );
        }
    }

    #[test]
    fn permission_and_alignment_errors_remain_backend_errors() {
        let (mut memory, audit) = MacOsArm64JitMemory::new_with_audit().unwrap();
        assert_eq!(
            memory
                .allocate(64, memory.page_size as u64 + 1, JITMemoryKind::Writable)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        FAILURE.with(|failure| failure.set(Some((Operation::Map, libc::EPERM))));
        assert_eq!(
            memory
                .allocate(64, 8, JITMemoryKind::Executable)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EPERM)
        );
        assert!(memory.allocations.is_empty());
        memory.allocate(64, 8, JITMemoryKind::ReadOnly).unwrap();
        FAILURE.with(|failure| failure.set(Some((Operation::Protect, libc::EPERM))));
        let error = memory.finalize(BranchProtection::None).unwrap_err();
        let ModuleError::Backend(error) = error else {
            panic!("expected contextual backend permission error");
        };
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().raw_os_error(),
            Some(libc::EPERM)
        );
        assert!(!memory.allocations[0].finalized);
        memory.finalize(BranchProtection::None).unwrap();
        assert!(memory.allocations[0].finalized);
        assert_eq!(audit.snapshot().rwx_mappings, 0);
    }

    #[test]
    fn readonly_slices_preserve_alignment_payloads_and_finalization_batches() {
        let (mut memory, audit) = MacOsArm64JitMemory::new_with_audit().unwrap();
        let first = memory.allocate(3, 1, JITMemoryKind::ReadOnly).unwrap();
        let aligned = memory.allocate(8, 8, JITMemoryKind::ReadOnly).unwrap();
        let zero = memory.allocate(0, 1, JITMemoryKind::ReadOnly).unwrap();
        let zero_again = memory.allocate(0, 1, JITMemoryKind::ReadOnly).unwrap();
        assert_eq!((aligned as usize) % 8, 0);
        assert_ne!(zero, zero_again);
        assert_eq!(memory.allocations.len(), 1);
        let page = memory.page_size;
        let large = memory.allocate(page, 8, JITMemoryKind::ReadOnly).unwrap();
        let tail = memory.allocate(7, 8, JITMemoryKind::ReadOnly).unwrap();
        assert_eq!(memory.allocations.len(), 2);
        // SAFETY: all returned ranges are live, disjoint and still writable.
        unsafe {
            first.write_bytes(0xa3, 3);
            aligned.cast::<u64>().write(0x123456789abcdef0);
            large.write(0xb4);
            large.add(page - 1).write(0xb5);
            tail.write_bytes(0xc6, 7);
        }
        memory.finalize(BranchProtection::None).unwrap();
        assert!(memory.readonly_current.is_none());
        assert!(memory.allocations.iter().all(|item| item.finalized));
        let later = memory.allocate(8, 8, JITMemoryKind::ReadOnly).unwrap();
        assert_eq!(memory.allocations.len(), 3);
        // SAFETY: only the newly allocated batch is written; earlier data are readonly.
        unsafe {
            later.write_bytes(0xd7, 8);
            assert_eq!(first.read(), 0xa3);
            assert_eq!(aligned.cast::<u64>().read(), 0x123456789abcdef0);
            assert_eq!(large.read(), 0xb4);
            assert_eq!(large.add(page - 1).read(), 0xb5);
            assert_eq!(tail.add(6).read(), 0xc6);
        }
        memory.finalize(BranchProtection::None).unwrap();
        // SAFETY: no code executes and no data pointer is used after this release.
        unsafe { memory.free_memory() };
        assert!(memory.allocations.is_empty() && memory.readonly_current.is_none());
        memory.allocate(8, 8, JITMemoryKind::ReadOnly).unwrap();
        memory.finalize(BranchProtection::None).unwrap();
        assert_eq!(audit.snapshot().rwx_mappings, 0);
    }

    #[test]
    fn readonly_write_child() {
        if std::env::var_os("RINZU_JIT_READONLY_WRITE").is_none() {
            return;
        }
        extern "C" fn rejected(_: libc::c_int) {
            // SAFETY: signal-safe termination of this private fault witness.
            unsafe { libc::_exit(0) };
        }
        let (mut memory, _) = MacOsArm64JitMemory::new_with_audit().unwrap();
        let first = memory.allocate(8, 8, JITMemoryKind::ReadOnly).unwrap();
        let second = memory.allocate(8, 8, JITMemoryKind::ReadOnly).unwrap();
        // SAFETY: the two ranges are writable until finalization.
        unsafe {
            first.write(1);
            second.write(2);
        }
        memory.finalize(BranchProtection::None).unwrap();
        // SAFETY: handlers only call _exit; a forbidden write must trap. The
        // child avoids crash reporting and never unwinds through a signal.
        unsafe {
            assert_eq!(first.read(), 1);
            assert_eq!(second.read(), 2);
            libc::signal(libc::SIGSEGV, rejected as *const () as libc::sighandler_t);
            libc::signal(libc::SIGBUS, rejected as *const () as libc::sighandler_t);
            first.write_volatile(9);
            libc::_exit(99);
        }
    }

    #[test]
    fn readonly_pages_keep_thousands_of_distinct_literals() {
        let (mut memory, _) = MacOsArm64JitMemory::new_with_audit().unwrap();
        let mut pointers = Vec::new();
        let mut addresses = std::collections::HashSet::new();
        for value in 0..4096_u64 {
            let pointer = memory.allocate(16, 8, JITMemoryKind::ReadOnly).unwrap();
            assert!(addresses.insert(pointer as usize));
            // SAFETY: these are 16 writable, 8-byte-aligned bytes in a distinct range.
            unsafe {
                pointer.cast::<u64>().write(value);
                pointer.add(8).cast::<u64>().write(!value);
            }
            pointers.push(pointer);
        }
        assert_eq!(
            memory.allocations.len(),
            (4096 * 16_usize).div_ceil(memory.page_size)
        );
        memory.finalize(BranchProtection::None).unwrap();
        for (value, pointer) in pointers.into_iter().enumerate() {
            // SAFETY: every mapping remains live and is now readonly.
            unsafe {
                assert_eq!(pointer.cast::<u64>().read(), value as u64);
                assert_eq!(pointer.add(8).cast::<u64>().read(), !(value as u64));
            }
        }
    }

    #[test]
    fn finalized_readonly_slices_reject_writes() {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "macos_jit_memory::tests::readonly_write_child",
                "--nocapture",
            ])
            .env("RINZU_JIT_READONLY_WRITE", "1")
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(0), "{result:?}");
    }

    #[test]
    #[ignore = "standalone performance probe; run via compare_runtime_costs.py"]
    fn readonly_allocation_cost_probe() {
        for count in [1_usize, 64, 4096] {
            let (mut memory, _) = MacOsArm64JitMemory::new_with_audit().unwrap();
            let mut pointers = Vec::with_capacity(count);
            let begin = Instant::now();
            for index in 0..count {
                let pointer = memory.allocate(16, 8, JITMemoryKind::ReadOnly).unwrap();
                // SAFETY: this provider just returned 16 writable bytes.
                unsafe { pointer.write_bytes(index as u8, 16) };
                pointers.push(pointer);
            }
            memory.finalize(BranchProtection::None).unwrap();
            let allocate_time = begin.elapsed();
            let mappings = memory.allocations.len();
            let bytes: usize = memory.allocations.iter().map(|item| item.length).sum();
            let mut checksum = 0_u64;
            for (index, pointer) in pointers.iter().copied().enumerate() {
                assert_eq!((pointer as usize) % 8, 0);
                // SAFETY: the mapping remains live and is now readonly.
                let contents = unsafe { std::slice::from_raw_parts(pointer, 16) };
                assert!(contents.iter().all(|byte| *byte == index as u8));
                checksum += contents.iter().map(|byte| u64::from(*byte)).sum::<u64>();
            }
            let begin = Instant::now();
            drop(memory);
            let elapsed = allocate_time + begin.elapsed();
            println!(
                "{}",
                serde_json::json!({
                    "case": format!("jit-readonly-{count}"), "operations": count,
                    "nanoseconds": elapsed.as_nanos(), "allocations": mappings,
                    "allocated_bytes": bytes, "allocation_kind": "vm-mapping", "checksum": checksum,
                })
            );
        }
    }
}
