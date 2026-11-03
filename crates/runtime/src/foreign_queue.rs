//! Shared C/Rust admitted-invocation record/FIFO primitive, not a dispatcher.
use core::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

pub type Notify = unsafe extern "C" fn(*mut c_void);
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QueueStats {
    pub live: usize,
    pub peak_live: usize,
    pub bytes: usize,
    pub peak_bytes: usize,
    pub queued: usize,
    pub peak_queued: usize,
    pub wake_pins: usize,
    pub created: u64,
    pub published: u64,
    pub received: u64,
    pub retired: u64,
}
impl QueueStats {
    pub fn is_baseline(self) -> bool {
        self.live == 0 && self.bytes == 0 && self.queued == 0 && self.wake_pins == 0
    }
}
struct Shared(NonNull<c_void>);
// SAFETY: producer access only copies ABI words, uses C locks/Condvars and
// touches stable admission/wake metadata. Owner/managed access is separate.
unsafe impl Send for Shared {}
// SAFETY: all shared C queue metadata and record results are synchronized.
unsafe impl Sync for Shared {}
impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: Owner's Drop requires the last reference and settled records.
        unsafe { rinzu_foreign_queue_drop(self.0.as_ptr()) };
    }
}
pub struct ForeignQueue {
    shared: Arc<Shared>,
    _owner: PhantomData<Rc<()>>,
}
#[derive(Clone)]
pub struct Producer {
    shared: Arc<Shared>,
}
impl ForeignQueue {
    pub(crate) fn raw_pointer(&self) -> *mut c_void {
        self.shared.0.as_ptr()
    }
    fn progress(&self, operation: u32, generation: u64) {
        // SAFETY: this !Send Owner is the sole progress-state writer.
        unsafe { rinzu_foreign_queue_progress(self.raw_pointer(), operation, generation) };
    }
    /// Classified by the runtime's Main task factory, never an SDK modifier.
    pub fn attach_main(&self) {
        self.progress(0, 0);
    }
    pub fn begin_loop(&self, generation: u64) {
        self.progress(1, generation);
    }
    pub fn end_loop(&self, generation: u64) {
        self.progress(2, generation);
    }
    pub fn enter_bounded(&self) {
        self.progress(3, 0);
    }
    pub fn leave_bounded(&self) {
        self.progress(4, 0);
    }
    /// # Safety
    /// Wake must be thread-safe, never unwind and its metadata context must
    /// remain valid until all producers and this Owner queue have settled.
    pub unsafe fn new(wake: Option<Notify>, context: *mut c_void) -> Self {
        // SAFETY: caller supplies a thread-safe pinned wake contract.
        let raw = unsafe { rinzu_foreign_queue_create(wake, context) };
        Self {
            shared: Arc::new(Shared(
                NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
            )),
            _owner: PhantomData,
        }
    }
    pub fn producer(&self) -> Producer {
        Producer {
            shared: Arc::clone(&self.shared),
        }
    }
    pub fn stats(&self) -> QueueStats {
        // SAFETY: Owner pins the queue and C copies synchronized metadata.
        unsafe { rinzu_foreign_queue_snapshot(self.shared.0.as_ptr()) }
    }
    pub fn pop(&self) -> Option<Invocation<'_>> {
        // SAFETY: !Send Owner is the sole consumer; queue ref transfers here.
        NonNull::new(unsafe { rinzu_foreign_queue_take(self.shared.0.as_ptr()) }).map(|record| {
            Invocation {
                record: Some(record),
                _queue: PhantomData,
            }
        })
    }
}
impl Drop for ForeignQueue {
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) != 1 || !self.stats().is_baseline() {
            // SAFETY: no output/allocation/unwind on an invalid lifetime.
            unsafe { libc::_exit(86) };
        }
    }
}
impl Producer {
    /// Notify only stable native wake metadata. The shared queue reference and
    /// C wake pin remain live through the entire notification.
    pub fn wake(&self) {
        // SAFETY: producers pin the queue; C locks and pins its wake target.
        unsafe { rinzu_foreign_queue_wake(self.shared.0.as_ptr()) };
    }
    /// # Safety
    /// A real call gate must already grant admission through result receipt.
    /// Receipt touches only thread-safe stable metadata and never unwinds.
    /// Pointer words' native storage stays valid/immutable until this returns;
    /// signature and Main/progress eligibility must be checked before calling.
    pub unsafe fn invoke(
        &self,
        callback: u64,
        generation: u64,
        words: &[u64],
        receipt: Option<Notify>,
        context: *mut c_void,
    ) -> u64 {
        // SAFETY: caller holds admission and native pointer lifetime; C never
        // resolves heap/root/ABI values on this producer thread.
        unsafe {
            rinzu_foreign_queue_invoke(
                self.shared.0.as_ptr(),
                callback,
                generation,
                words.as_ptr(),
                words.len(),
                receipt,
                context,
            )
        }
    }
}
pub struct Invocation<'queue> {
    record: Option<NonNull<c_void>>,
    _queue: PhantomData<&'queue ForeignQueue>,
}
impl Invocation<'_> {
    fn identity_field(&self, field: u32) -> u64 {
        // SAFETY: Owner's transferred queue ref pins this executing record.
        unsafe { rinzu_foreign_queue_identity(self.record.unwrap().as_ptr(), field) }
    }
    pub fn identity(&self) -> u64 {
        self.identity_field(0)
    }
    pub fn sequence(&self) -> u64 {
        self.identity_field(1)
    }
    pub fn callback(&self) -> u64 {
        self.identity_field(2)
    }
    pub fn generation(&self) -> u64 {
        self.identity_field(3)
    }
    pub fn words(&self) -> &[u64] {
        let mut count = 0;
        // SAFETY: immutable payload is pinned by this Owner invocation.
        let words = unsafe { rinzu_foreign_queue_words(self.record.unwrap().as_ptr(), &mut count) };
        // SAFETY: C returns the exact count of its nonmoving inline payload.
        unsafe { std::slice::from_raw_parts(words, count) }
    }
    pub fn complete(mut self, result: u64) {
        let record = self.record.take().unwrap();
        // SAFETY: publish once then release Owner reference; native receiver
        // separately copies the result and settles admission before reclamation.
        unsafe { rinzu_foreign_queue_complete(record.as_ptr(), result) };
    }
}
impl Drop for Invocation<'_> {
    fn drop(&mut self) {
        if self.record.is_some() {
            // SAFETY: an admitted invocation cannot disappear or synthesize a result.
            unsafe { libc::_exit(86) };
        }
    }
}
unsafe extern "C" {
    fn rinzu_foreign_queue_progress(queue: *mut c_void, operation: u32, generation: u64);
    fn rinzu_foreign_queue_create(wake: Option<Notify>, context: *mut c_void) -> *mut c_void;
    fn rinzu_foreign_queue_drop(queue: *mut c_void);
    fn rinzu_foreign_queue_snapshot(queue: *mut c_void) -> QueueStats;
    fn rinzu_foreign_queue_wake(queue: *mut c_void);
    fn rinzu_foreign_queue_take(queue: *mut c_void) -> *mut c_void;
    fn rinzu_foreign_queue_invoke(
        queue: *mut c_void,
        callback: u64,
        generation: u64,
        words: *const u64,
        count: usize,
        receipt: Option<Notify>,
        context: *mut c_void,
    ) -> u64;
    fn rinzu_foreign_queue_complete(record: *mut c_void, result: u64);
    fn rinzu_foreign_queue_words(record: *mut c_void, count: *mut usize) -> *const u64;
    fn rinzu_foreign_queue_identity(record: *mut c_void, field: u32) -> u64;
}
