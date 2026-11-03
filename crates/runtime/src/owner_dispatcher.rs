//! Runtime-owned macOS RunLoop source, observer and bounded Owner turns.
use core::ffi::c_void;
use std::ptr::NonNull;

use crate::foreign_queue::ForeignQueue;
pub type Step = unsafe extern "C" fn(*mut c_void, u32) -> i32;
pub type Ready = unsafe extern "C" fn(*mut c_void, u32) -> i32;
pub type CanEnter = unsafe extern "C" fn(*mut c_void) -> i32;
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DispatcherStats {
    pub wakes: u64,
    pub signals: u64,
    pub turns: u64,
    pub activations: u64,
    pub foreign: u64,
    pub managed: u64,
    pub rearms: u64,
    pub deferred: u64,
    pub max_activations: usize,
}
pub struct OwnerDispatcher {
    raw: NonNull<c_void>,
    queue: ForeignQueue,
}
impl OwnerDispatcher {
    /// # Safety
    /// Hooks are Owner-only, perform at most one activation and never unwind.
    /// Context/code remain pinned until all calls, receipts and wakes finish.
    /// Ready is a side-effect-free snapshot. CanEnter checks managed/native
    /// activation state without running user code. No SDK owns these hooks.
    pub unsafe fn new(
        step: Option<Step>,
        ready: Option<Ready>,
        can_enter: Option<CanEnter>,
        context: *mut c_void,
    ) -> Self {
        // SAFETY: the runtime installs its wake before any native invocation.
        let queue = unsafe { ForeignQueue::new(None, std::ptr::null_mut()) };
        queue.attach_main();
        // SAFETY: caller supplies pinned Owner hook contracts.
        let raw = unsafe {
            rinzu_owner_dispatcher_new(queue.raw_pointer(), step, ready, can_enter, context)
        };
        Self {
            raw: NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
            queue,
        }
    }
    pub fn queue(&self) -> &ForeignQueue {
        &self.queue
    }
    /// # Safety
    /// Install once before native entry. Hooks/context obey the same lifetime,
    /// Owner-only and non-unwinding contracts as `new`.
    pub unsafe fn set_hooks(
        &self,
        step: Step,
        ready: Ready,
        can_enter: CanEnter,
        context: *mut c_void,
    ) {
        // SAFETY: caller pins independent Owner context before publication.
        unsafe {
            rinzu_owner_dispatcher_set_hooks(self.raw.as_ptr(), step, ready, can_enter, context)
        };
    }
    pub fn wake(&self) {
        // SAFETY: !Send Owner holds the dispatcher and queue.
        unsafe { rinzu_owner_dispatcher_wake(self.raw.as_ptr()) };
    }
    pub fn pump(&self) {
        // SAFETY: !Send Owner is the only consumer of its turns.
        unsafe { rinzu_owner_dispatcher_pump(self.raw.as_ptr()) };
    }
    pub fn stats(&self) -> DispatcherStats {
        // SAFETY: Owner pins the synchronized dispatcher metadata.
        unsafe { rinzu_owner_dispatcher_stats(self.raw.as_ptr()) }
    }
    #[cfg(feature = "test-dispatcher-probe")]
    pub fn test_return_after_published(&self, count: usize) {
        // SAFETY: test-only Owner control is installed before native entry.
        unsafe { rinzu_owner_dispatcher_test_return_after_published(self.raw.as_ptr(), count) };
    }
}
impl Drop for OwnerDispatcher {
    fn drop(&mut self) {
        // SAFETY: C validates ended loop and complete queue/wake baseline before
        // removing the RunLoop hooks. The queue then drops on the same Owner.
        unsafe { rinzu_owner_dispatcher_dispose(self.raw.as_ptr()) };
    }
}
unsafe extern "C" {
    fn rinzu_owner_dispatcher_new(
        queue: *mut c_void,
        step: Option<Step>,
        ready: Option<Ready>,
        can_enter: Option<CanEnter>,
        context: *mut c_void,
    ) -> *mut c_void;
    fn rinzu_owner_dispatcher_wake(dispatcher: *mut c_void);
    fn rinzu_owner_dispatcher_set_hooks(
        dispatcher: *mut c_void,
        step: Step,
        ready: Ready,
        can_enter: CanEnter,
        context: *mut c_void,
    );
    fn rinzu_owner_dispatcher_pump(dispatcher: *mut c_void);
    fn rinzu_owner_dispatcher_stats(dispatcher: *mut c_void) -> DispatcherStats;
    fn rinzu_owner_dispatcher_dispose(dispatcher: *mut c_void);
    #[cfg(feature = "test-dispatcher-probe")]
    fn rinzu_owner_dispatcher_test_return_after_published(dispatcher: *mut c_void, count: usize);
}
