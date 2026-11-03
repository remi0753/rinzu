//! Native-only completion metadata. The SDK retains storage until settlement.
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

#[repr(C)]
pub struct NativeCompletion {
    pub version: u32,
    pub size: u32,
    pub done: AtomicU32,
    pub cancelled: AtomicU32,
}

/// Optional native_completion.h v2 extension; v1 remains exactly 16 bytes.
#[repr(C)]
pub struct NotifiedNativeCompletion {
    pub base: NativeCompletion,
    pub notification: AtomicUsize,
}
impl Default for NotifiedNativeCompletion {
    fn default() -> Self {
        Self {
            base: NativeCompletion {
                version: 2,
                size: size_of::<Self>() as u32,
                done: AtomicU32::new(0),
                cancelled: AtomicU32::new(0),
            },
            notification: AtomicUsize::new(0),
        }
    }
}
/// Optional scalar receipt; the v1/v2 layouts stay unchanged.
#[repr(C)]
pub struct TransferNativeCompletion {
    pub completion: NotifiedNativeCompletion,
    pub result: i64,
}
impl Default for TransferNativeCompletion {
    fn default() -> Self {
        let mut completion = NotifiedNativeCompletion::default();
        completion.base.version = 3;
        completion.base.size = size_of::<Self>() as u32;
        Self {
            completion,
            result: 0,
        }
    }
}
const DONE: usize = 1;

#[repr(C)]
struct NativeCompletionWakeHeader {
    notify: unsafe extern "C" fn(*mut NativeCompletionWakeHeader),
    release: unsafe extern "C" fn(*mut NativeCompletionWakeHeader),
}
impl NotifiedNativeCompletion {
    /// Release-publish a producer's result with no carrier access after the
    /// final exchange. The retained wake target may outlive the carrier.
    ///
    /// # Safety
    /// `raw` must name a valid v2 carrier through its final exchange, published
    /// exactly once after the producer's last payload access. Registration must
    /// follow native_completion.h's retained native target protocol.
    pub unsafe fn publish(raw: *mut Self) {
        let before = {
            // SAFETY: the producer pins its carrier through this final access.
            let completion = unsafe { &*raw };
            completion.base.done.store(1, Ordering::Release);
            completion.notification.swap(DONE, Ordering::AcqRel)
        };
        if before > DONE {
            let target = before as *mut NativeCompletionWakeHeader;
            // SAFETY: the CAS winner transferred one target reference. Neither
            // callback touches the carrier; release consumes the reference.
            unsafe {
                ((*target).notify)(target);
                ((*target).release)(target);
            }
        }
    }
}

// The first two words are the C ABI rinzu_native_completion_wake_t. Each CAS
// winner transfers an Arc reference to the producer, which notifies/releases
// after its final carrier access. A late notification may fail to send to the
// already shut down reactor; it retains no runtime/heap/SDK carrier pointer.
#[repr(C)]
pub(crate) struct NativeCompletionWake {
    header: NativeCompletionWakeHeader,
    wake: Box<dyn Fn() + Send + Sync>,
}
impl NativeCompletionWake {
    pub(crate) fn new(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        unsafe extern "C" fn notify(raw: *mut NativeCompletionWakeHeader) {
            // SAFETY: the winning registration retains an Arc through release.
            unsafe { ((*raw.cast::<NativeCompletionWake>()).wake)() };
        }
        unsafe extern "C" fn release(raw: *mut NativeCompletionWakeHeader) {
            // SAFETY: consumes exactly the reference transferred by registration.
            drop(unsafe { Arc::from_raw(raw.cast::<NativeCompletionWake>()) });
        }
        Arc::new(Self {
            header: NativeCompletionWakeHeader { notify, release },
            wake: Box::new(wake),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CompletionPointer(usize);
impl CompletionPointer {
    /// # Safety
    /// The pointer must name a valid, aligned native_completion.h v1/v2/v3 object,
    /// pinned through every use of this pointer and all copies, including admitted
    /// waits and control requests. Its payload must
    /// no longer be accessed by the producer after v1 done or the v2 final
    /// notification exchange. A v2 carrier has one runtime notification domain.
    pub unsafe fn new(address: usize) -> Self {
        if address == 0 || !address.is_multiple_of(align_of::<NativeCompletion>()) {
            protocol_fatal();
        }
        let pointer = Self(address);
        // SAFETY: the caller establishes the protocol's pinned readable object.
        let value = unsafe { &*(address as *const NativeCompletion) };
        if !match value.version {
            1 => value.size == size_of::<NativeCompletion>() as u32,
            2 => {
                value.size == size_of::<NotifiedNativeCompletion>() as u32
                    && address.is_multiple_of(align_of::<NotifiedNativeCompletion>())
            }
            3 => {
                value.size == size_of::<TransferNativeCompletion>() as u32
                    && address.is_multiple_of(align_of::<TransferNativeCompletion>())
            }
            _ => false,
        } {
            protocol_fatal();
        }
        pointer
    }
    fn value(self) -> &'static NativeCompletion {
        // SAFETY: construction requires pinning through this admitted wait;
        // only immutable metadata and ABI-compatible atomic words are read.
        unsafe { &*(self.0 as *const NativeCompletion) }
    }
    pub fn is_done(self) -> bool {
        if self.supports_notification() {
            self.notified().notification.load(Ordering::Acquire) == DONE
        } else {
            self.value().done.load(Ordering::Acquire) != 0
        }
    }
    pub fn supports_notification(self) -> bool {
        matches!(self.value().version, 2 | 3)
    }
    pub fn supports_transfer(self) -> bool {
        self.value().version == 3
    }
    pub fn require_transfer(self) {
        if !self.supports_transfer() {
            protocol_fatal();
        }
    }
    /// Copy during physical publication, while the SDK still pins the carrier.
    pub fn transfer_result(self) -> i64 {
        if !self.supports_transfer() || !self.is_done() {
            protocol_fatal();
        }
        // SAFETY: validated v3 size/alignment and acquire publication. Byte
        // access permits a provider's own record with the same C prefix.
        unsafe {
            ((self.0 + std::mem::offset_of!(TransferNativeCompletion, result)) as *const i64).read()
        }
    }
    fn notified(self) -> &'static NotifiedNativeCompletion {
        // SAFETY: callers first check the v2/v3 prefix; construction validates its size and
        // alignment, and the admission pins the object until physical return.
        unsafe { &*(self.0 as *const NotifiedNativeCompletion) }
    }
    pub(crate) fn register_wake(self, target: &Arc<NativeCompletionWake>) {
        let raw = Arc::into_raw(Arc::clone(target)) as usize;
        if let Err(before) = self.notified().notification.compare_exchange(
            0,
            raw,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            // SAFETY: the failed CAS did not transfer this Arc to a producer.
            drop(unsafe { Arc::from_raw(raw as *const NativeCompletionWake) });
            if before != DONE && before != raw {
                protocol_fatal();
            }
        }
    }
    pub fn cancel(self) {
        self.value().cancelled.store(1, Ordering::Release);
    }
}
fn protocol_fatal() -> ! {
    // SAFETY: an invalid trusted native protocol cannot unwind into managed code.
    unsafe { libc::_exit(86) }
}
