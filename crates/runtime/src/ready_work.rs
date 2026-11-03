//! Native-only physical readiness. A late notification cannot recreate work
//! that its Origin Owner has already received or explicitly abandoned.
use crate::{failure::SourcePosition, native_diagnostics::ActivityMonitor};
use core::ffi::{CStr, c_void};
use std::{marker::PhantomData, ptr::NonNull, rc::Rc, sync::Arc};
struct Shared {
    raw: NonNull<c_void>,
    monitor: ActivityMonitor,
}
// SAFETY: only synchronized native readiness/timer metadata crosses threads.
unsafe impl Send for Shared {}
// SAFETY: the C gate separates publication from Owner receipt; no heap pointer.
unsafe impl Sync for Shared {}
impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: the last Arc is quiescent; C requires prior Owner settlement.
        unsafe { rinzu_ready_work_dispose(self.raw.as_ptr()) };
        let _ = &self.monitor;
    }
}
pub struct ReadyReceipt {
    shared: Arc<Shared>,
    received: bool,
    _owner: PhantomData<Rc<()>>,
}
#[derive(Clone)]
pub struct ReadyPublication(Arc<Shared>);
impl std::fmt::Debug for ReadyPublication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadyPublication")
            .finish_non_exhaustive()
    }
}
impl ActivityMonitor {
    pub fn prepare_ready(
        &self,
        symbol: &CStr,
        source: SourcePosition,
        owner: u64,
        depth: u64,
    ) -> ReadyReceipt {
        // SAFETY: C copies metadata; the preparing thread is the board Owner.
        let raw = unsafe {
            rinzu_ready_work_prepare(
                self.raw_pointer(),
                symbol.as_ptr(),
                source.file,
                source.start_byte,
                source.end_byte,
                owner,
                depth,
            )
        };
        ReadyReceipt {
            shared: Arc::new(Shared {
                raw: NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
                monitor: self.clone(),
            }),
            received: false,
            _owner: PhantomData,
        }
    }
}
impl ReadyReceipt {
    pub fn publication(&self) -> ReadyPublication {
        ReadyPublication(self.shared.clone())
    }
    /// Call after taking the real packet, or after proving admission never
    /// started its physical producer. Cancellation alone is not a receipt.
    pub fn receive(mut self) {
        // SAFETY: !Send receiver is confined to the preparing Owner, once.
        unsafe { rinzu_ready_work_receive(self.shared.raw.as_ptr()) };
        self.received = true;
    }
}
impl Drop for ReadyReceipt {
    fn drop(&mut self) {
        if !self.received {
            // SAFETY: no allocation/unwind/cleanup for an unsettled receiver.
            unsafe { libc::_exit(86) };
        }
    }
}
impl ReadyPublication {
    /// Publish once after physical completion/packet publication. It is legal
    /// for Owner receipt to win this race; that notification stays settled.
    pub fn publish(&self) {
        // SAFETY: Arc pins native metadata; the C gate rejects a second publish.
        unsafe { rinzu_ready_work_publish(self.0.raw.as_ptr()) };
    }
}
unsafe extern "C" {
    fn rinzu_ready_work_prepare(
        board: *mut c_void,
        symbol: *const core::ffi::c_char,
        file: u64,
        start: u32,
        end: u32,
        owner: u64,
        depth: u64,
    ) -> *mut c_void;
    fn rinzu_ready_work_publish(work: *mut c_void);
    fn rinzu_ready_work_receive(work: *mut c_void);
    fn rinzu_ready_work_dispose(work: *mut c_void);
}
