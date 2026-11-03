//! Native-only, prepared interval metadata monitored independently of the Owner.
use crate::failure::SourcePosition;
use core::ffi::{CStr, c_void};
use std::{
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    sync::{Arc, Mutex},
};
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum ActivityKind {
    Managed,
    Native,
    ForeignWait,
    OwnerStall,
    Shutdown,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ActivityStats {
    pub prepared: u64,
    pub ended: u64,
    pub warnings: [u64; 5],
    pub live: usize,
    pub active: usize,
    pub bytes: usize,
    pub warning_pins: usize,
    pub ready_live: usize,
    pub ready_bytes: usize,
    pub wait_live: usize,
    pub wait_bytes: usize,
    /// Retired record storage, distinct from live diagnostic intervals.
    pub cached_records: usize,
    pub cached_bytes: usize,
}
impl ActivityStats {
    /// No live interval, ready/wait record or warning pin remains. A bounded
    /// record cache may still retain native storage, exposed in cached_*.
    pub fn is_quiescent(&self) -> bool {
        self.prepared == self.ended
            && self.live == 0
            && self.active == 0
            && self.bytes == 0
            && self.warning_pins == 0
            && self.ready_live == 0
            && self.ready_bytes == 0
            && self.wait_live == 0
            && self.wait_bytes == 0
    }
    /// No logical record or retired cache storage remains (board excluded).
    pub fn is_baseline(&self) -> bool {
        self.is_quiescent() && self.cached_records == 0 && self.cached_bytes == 0
    }
}
struct Board(NonNull<c_void>);
// SAFETY: C synchronizes all native metadata. It holds no managed pointers and
// prepares a separate creator-bound interval for each physical caller.
unsafe impl Send for Board {}
// SAFETY: snapshots and record publication serialize under the native gate.
unsafe impl Sync for Board {}
impl Drop for Board {
    fn drop(&mut self) {
        // SAFETY: each interval pins this Arc through creator-thread settlement;
        // C joins the independent watchdog before destroying its native target.
        unsafe { rinzu_diagnostic_board_dispose(self.0.as_ptr()) };
    }
}
#[derive(Clone)]
pub struct ActivityMonitor(Arc<Board>);
/// A ticket carries only this native metadata link across Origin and Worker.
/// Registration pins the board until the physical Worker finishes; removal
/// serializes with requests before the final board/watchdog disposal.
#[derive(Clone, Default)]
pub struct NativeCancellation(Arc<Mutex<CancellationTarget>>);
#[derive(Default)]
struct CancellationTarget {
    requested: bool,
    monitor: Option<ActivityMonitor>,
}
impl std::fmt::Debug for NativeCancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let target = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        formatter
            .debug_struct("NativeCancellation")
            .field("requested", &target.requested)
            .field("registered", &target.monitor.is_some())
            .finish()
    }
}
pub struct NativeCancellationRegistration {
    target: NativeCancellation,
    _creator: PhantomData<Rc<()>>,
}
impl NativeCancellation {
    pub fn request(&self) {
        let mut target = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        target.requested = true;
        if let Some(monitor) = &target.monitor {
            monitor.request_shutdown();
        }
    }
    pub fn register(&self, monitor: ActivityMonitor) -> NativeCancellationRegistration {
        let mut target = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if target.monitor.is_some() {
            // SAFETY: a one-shot operation cannot own two physical Workers.
            unsafe { libc::_exit(86) };
        }
        if target.requested {
            monitor.request_shutdown();
        }
        target.monitor = Some(monitor);
        NativeCancellationRegistration {
            target: self.clone(),
            _creator: PhantomData,
        }
    }
}
impl Drop for NativeCancellationRegistration {
    fn drop(&mut self) {
        let monitor = self
            .target
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .monitor
            .take();
        // Joining a final watchdog never holds the request/registration gate.
        drop(monitor);
    }
}
impl Default for ActivityMonitor {
    fn default() -> Self {
        Self::new()
    }
}
impl ActivityMonitor {
    pub fn wait_for(
        &self,
        task: u64,
        dependency: u64,
        symbol: &CStr,
        source: SourcePosition,
        owner: u64,
        depth: u64,
    ) -> WaitDependency {
        // SAFETY: C copies only native IDs/source/symbol and requires the
        // physical Owner. The guard pins the board through actual task resume.
        let raw = unsafe {
            rinzu_diagnostic_wait_begin(
                self.raw_pointer(),
                task,
                dependency,
                symbol.as_ptr(),
                source.file,
                source.start_byte,
                source.end_byte,
                owner,
                depth,
            )
        };
        WaitDependency {
            raw: NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
            board: self.0.clone(),
            _creator: PhantomData,
        }
    }
    pub(crate) fn raw_pointer(&self) -> *mut c_void {
        self.0.0.as_ptr()
    }
    pub fn new() -> Self {
        // SAFETY: creates synchronized native metadata and a heap-free watchdog.
        let raw = unsafe { rinzu_diagnostic_board_new() };
        Self(Arc::new(Board(
            NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
        )))
    }
    pub fn prepare(
        &self,
        kind: ActivityKind,
        symbol: &CStr,
        source: SourcePosition,
        owner: u64,
        depth: u64,
    ) -> Activity {
        // SAFETY: C copies the complete symbol while its input is live. Position,
        // identity and depth are native values, never managed addresses.
        let raw = unsafe {
            rinzu_diagnostic_prepare(
                self.0.0.as_ptr(),
                kind as u32,
                symbol.as_ptr(),
                source.file,
                source.start_byte,
                source.end_byte,
                owner,
                depth,
            )
        };
        Activity {
            raw: NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
            board: self.0.clone(),
            _creator: PhantomData,
        }
    }
    pub fn stats(&self) -> ActivityStats {
        // SAFETY: Arc pins native metadata; the synchronized snapshot reads no heap.
        unsafe { rinzu_diagnostic_stats(self.0.0.as_ptr()) }
    }
    pub fn prepare_native(
        &self,
        symbol: &CStr,
        source: SourcePosition,
        owner: u64,
        depth: u64,
    ) -> Activity {
        // SAFETY: both native-only intervals copy metadata before physical entry.
        let raw = unsafe {
            rinzu_diagnostic_prepare_native(
                self.raw_pointer(),
                symbol.as_ptr(),
                source.file,
                source.start_byte,
                source.end_byte,
                owner,
                depth,
            )
        };
        Activity {
            raw: NonNull::new(raw).unwrap_or_else(|| crate::allocation::fatal()),
            board: self.0.clone(),
            _creator: PhantomData,
        }
    }
    pub fn request_shutdown(&self) {
        // SAFETY: Arc pins the native board; its gate arms only active native
        // pairs. This cross-thread observation never touches an Owner heap.
        unsafe { rinzu_diagnostic_request_shutdown(self.raw_pointer()) };
    }
    pub fn owner_turn(&self) {
        // SAFETY: C requires the board's physical Owner and reads only metadata.
        unsafe { rinzu_diagnostic_owner_turn(self.raw_pointer()) };
    }
}
pub struct Activity {
    raw: NonNull<c_void>,
    board: Arc<Board>,
    _creator: PhantomData<Rc<()>>,
}
pub struct WaitDependency {
    raw: NonNull<c_void>,
    board: Arc<Board>,
    _creator: PhantomData<Rc<()>>,
}
impl Drop for WaitDependency {
    fn drop(&mut self) {
        // SAFETY: the !Send guard remains on the physical Owner. Removing a
        // wait never completes its Future or releases an admission/managed root.
        unsafe { rinzu_diagnostic_wait_end(self.raw.as_ptr()) };
        let _ = &self.board;
    }
}
impl Activity {
    pub fn start(&self) {
        // SAFETY: !Send interval remains on its preparer's physical thread.
        unsafe { rinzu_diagnostic_start(self.raw.as_ptr()) };
    }
}
impl Drop for Activity {
    fn drop(&mut self) {
        // SAFETY: C unlinks once, retaining an independent warning pin if needed;
        // the Arc outlives this creator-thread settlement and watchdog emission.
        unsafe { rinzu_diagnostic_end(self.raw.as_ptr()) };
        let _ = &self.board;
    }
}
unsafe extern "C" {
    fn rinzu_diagnostic_wait_begin(
        board: *mut c_void,
        task: u64,
        dependency: u64,
        symbol: *const core::ffi::c_char,
        file: u64,
        start: u32,
        end: u32,
        owner: u64,
        depth: u64,
    ) -> *mut c_void;
    fn rinzu_diagnostic_wait_end(wait: *mut c_void);
    fn rinzu_diagnostic_prepare_native(
        board: *mut c_void,
        symbol: *const core::ffi::c_char,
        file: u64,
        start: u32,
        end: u32,
        owner: u64,
        depth: u64,
    ) -> *mut c_void;
    fn rinzu_diagnostic_request_shutdown(board: *mut c_void);
    fn rinzu_diagnostic_board_new() -> *mut c_void;
    fn rinzu_diagnostic_prepare(
        board: *mut c_void,
        kind: u32,
        symbol: *const core::ffi::c_char,
        file: u64,
        start_byte: u32,
        end_byte: u32,
        owner: u64,
        depth: u64,
    ) -> *mut c_void;
    fn rinzu_diagnostic_start(activity: *mut c_void);
    fn rinzu_diagnostic_end(activity: *mut c_void);
    fn rinzu_diagnostic_stats(board: *mut c_void) -> ActivityStats;
    fn rinzu_diagnostic_board_dispose(board: *mut c_void);
    fn rinzu_diagnostic_owner_turn(board: *mut c_void);
}
