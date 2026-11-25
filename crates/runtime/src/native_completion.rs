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

/// One SDK reference returned by the final publication exchange.
#[cfg(feature = "test-native-lifetime-trace")]
pub struct CapturedNativeWake(std::ptr::NonNull<NativeCompletionWakeHeader>);

#[cfg(feature = "test-native-lifetime-trace")]
impl CapturedNativeWake {
    #[must_use]
    pub fn address(&self) -> usize {
        self.0.as_ptr() as usize
    }

    pub fn notify(&self) {
        // SAFETY: this guard keeps the transferred SDK reference live.
        unsafe { (self.0.as_ref().notify)(self.0.as_ptr()) };
    }
}

#[cfg(feature = "test-native-lifetime-trace")]
impl Drop for CapturedNativeWake {
    fn drop(&mut self) {
        // SAFETY: the exchange transferred one reference, consumed here once.
        unsafe { (self.0.as_ref().release)(self.0.as_ptr()) };
    }
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
        // SAFETY: this public entry establishes the same pin/publication contract.
        unsafe {
            Self::publish_inner(
                raw,
                #[cfg(feature = "test-native-lifetime-trace")]
                None,
            )
        };
    }
    /// Observe the real producer stores without changing the carrier ABI.
    /// # Safety
    /// The caller must satisfy the same requirements as [`Self::publish`].
    #[cfg(feature = "test-native-lifetime-trace")]
    pub unsafe fn publish_with_lifetime_trace(
        raw: *mut Self,
        probe: &crate::lifetime_trace::Probe,
    ) {
        // SAFETY: forwarded producer contract; the probe is independent of the carrier.
        unsafe { Self::publish_inner(raw, Some(probe)) };
    }
    unsafe fn publish_inner(
        raw: *mut Self,
        #[cfg(feature = "test-native-lifetime-trace")] probe: Option<&crate::lifetime_trace::Probe>,
    ) {
        // SAFETY: forwarded producer contract through the final exchange.
        let target = unsafe {
            Self::exchange_wake(
                raw,
                #[cfg(feature = "test-native-lifetime-trace")]
                probe,
            )
        };
        if let Some(target) = target {
            // SAFETY: the exchange supplied one reference. The callbacks use
            // only the target and its release consumes that reference.
            unsafe {
                (target.as_ref().notify)(target.as_ptr());
                (target.as_ref().release)(target.as_ptr());
            }
        }
    }

    /// Publish and inspect a retained reference before deciding whether a
    /// shared SDK batch needs this reference's notification.
    /// # Safety
    /// The caller must satisfy the same requirements as [Self::publish].
    #[cfg(feature = "test-native-lifetime-trace")]
    pub unsafe fn capture_with_lifetime_trace(
        raw: *mut Self,
        probe: &crate::lifetime_trace::Probe,
    ) -> Option<CapturedNativeWake> {
        // SAFETY: the caller supplies the pinned, uniquely published generation.
        unsafe { Self::exchange_wake(raw, Some(probe)) }.map(CapturedNativeWake)
    }

    unsafe fn exchange_wake(
        raw: *mut Self,
        #[cfg(feature = "test-native-lifetime-trace")] probe: Option<&crate::lifetime_trace::Probe>,
    ) -> Option<std::ptr::NonNull<NativeCompletionWakeHeader>> {
        let before = {
            // SAFETY: the producer pins its carrier through this final access.
            let completion = unsafe { &*raw };
            completion.base.done.store(1, Ordering::Release);
            #[cfg(feature = "test-native-lifetime-trace")]
            if let Some(p) = probe {
                p.record("sdk_prefix", 0, 0, 0);
            }
            completion.notification.swap(DONE, Ordering::AcqRel)
        };
        #[cfg(feature = "test-native-lifetime-trace")]
        if let Some(p) = probe {
            p.record("sdk_exchange", 0, i64::from(before > DONE), 0);
        }
        if before > DONE {
            std::ptr::NonNull::new(before as *mut NativeCompletionWakeHeader)
        } else {
            None
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
    #[cfg(feature = "test-native-lifetime-trace")]
    observer: Option<Arc<crate::lifetime_trace::Probe>>,
}
impl NativeCompletionWake {
    pub(crate) fn new(wake: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        unsafe extern "C" fn notify(raw: *mut NativeCompletionWakeHeader) {
            // SAFETY: the winning registration retains an Arc through release.
            let target = unsafe { &*raw.cast::<NativeCompletionWake>() };
            #[cfg(feature = "test-native-lifetime-trace")]
            if let Some(p) = &target.observer {
                p.record("notify_enter", 0, 0, 0);
            }
            (target.wake)();
            #[cfg(feature = "test-native-lifetime-trace")]
            if let Some(p) = &target.observer {
                p.record("notify_leave", 0, 0, 0);
            }
        }
        unsafe extern "C" fn release(raw: *mut NativeCompletionWakeHeader) {
            // SAFETY: consumes exactly the reference transferred by registration.
            let target = unsafe { Arc::from_raw(raw.cast::<NativeCompletionWake>()) };
            #[cfg(feature = "test-native-lifetime-trace")]
            let observer = target.observer.clone();
            #[cfg(feature = "test-native-lifetime-trace")]
            if let Some(p) = &target.observer {
                p.domain_event("producer_refs", 0, raw as usize, Arc::strong_count(&target));
                p.record("producer_release", 0, 0, 0);
            }
            drop(target);
            #[cfg(feature = "test-native-lifetime-trace")]
            if let Some(p) = observer {
                p.record("producer_released", 0, 0, 0);
            }
        }
        Arc::new(Self {
            header: NativeCompletionWakeHeader { notify, release },
            wake: Box::new(wake),
            #[cfg(feature = "test-native-lifetime-trace")]
            observer: None,
        })
    }
    #[cfg(feature = "test-native-lifetime-trace")]
    pub(crate) fn observe(
        target: &mut Arc<Self>,
        observer: Option<Arc<crate::lifetime_trace::Probe>>,
    ) {
        Arc::get_mut(target).unwrap().observer = observer;
    }
}
#[cfg(feature = "test-native-lifetime-trace")]
impl Drop for NativeCompletionWake {
    fn drop(&mut self) {
        if let Some(p) = &self.observer {
            p.domain_event(
                "target_identity_free",
                0,
                std::ptr::from_ref(self) as usize,
                0,
            );
            p.record("target_free", 0, 0, 0);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CompletionPointer(usize);
impl CompletionPointer {
    #[cfg(feature = "test-native-lifetime-trace")]
    pub(crate) fn address(self) -> usize {
        self.0
    }
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
    pub(crate) fn register_wake(self, target: &Arc<NativeCompletionWake>, _operation: u64) -> bool {
        #[cfg(feature = "test-native-lifetime-trace")]
        let trace = |kind, a, b| {
            if let Some(p) = &target.observer {
                p.record(kind, _operation, a, b);
            }
        };
        #[cfg(feature = "test-native-lifetime-trace")]
        if let Some(p) = &target.observer {
            p.bind_wake(_operation, Arc::as_ptr(target) as usize);
        }
        #[cfg(feature = "test-native-lifetime-trace")]
        trace("register_begin", 0, 0);
        let raw = Arc::into_raw(Arc::clone(target)) as usize;
        #[cfg(feature = "test-native-lifetime-trace")]
        trace("register_retain", 0, 0);
        let result = self.notified().notification.compare_exchange(
            0,
            raw,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        #[cfg(feature = "test-native-lifetime-trace")]
        trace(
            "register_cas",
            i64::from(result.is_ok()),
            result
                .as_ref()
                .err()
                .map_or(0, |before| if *before == DONE { 1 } else { 2 }),
        );
        if let Err(before) = result {
            // SAFETY: the failed CAS did not transfer this Arc to a producer.
            drop(unsafe { Arc::from_raw(raw as *const NativeCompletionWake) });
            #[cfg(feature = "test-native-lifetime-trace")]
            if let Some(p) = &target.observer {
                p.domain_event("rollback_refs", _operation, raw, Arc::strong_count(target));
            }
            #[cfg(feature = "test-native-lifetime-trace")]
            trace("register_drop", 0, 0);
            if before != DONE && before != raw {
                protocol_fatal();
            }
            false
        } else {
            true
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
