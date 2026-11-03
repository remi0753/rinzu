//! Precompiled, process-lifetime callback addresses for the source ABI.
use crate::data::{ManagedData, ManagedRoots};
use crate::heap::{CollectionKind, DomainHeap};
use core::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;

pub type PointerU64Handler = unsafe extern "C" fn(*mut c_void, *const u8) -> u64;
pub type PointerU64Entry = unsafe extern "C" fn(*const u8) -> u64;
pub type FrameHandler = unsafe extern "C" fn(*mut c_void, *const CallbackFrame) -> u64;

#[repr(C)]
pub struct CallbackFrame {
    pub integers: [u64; 8],
    pub floats: [u64; 8],
    pub stack: *const u8,
}
#[derive(Default)]
#[repr(C)]
pub struct CallbackAbiCursor {
    pub integers: usize,
    pub floats: usize,
    pub stack: usize,
}
impl CallbackFrame {
    /// Decode one parameter of the validated fixed physical signature.
    ///
    /// # Safety
    /// `self` must be the currently admitted bridge frame, and `scalar`/cursor
    /// must follow that entry's exact signature; None denotes one pointer.
    pub unsafe fn word(
        &self,
        cursor: &mut CallbackAbiCursor,
        scalar: Option<crate::scalar::ScalarKind>,
    ) -> u64 {
        // SAFETY: the caller supplies the exact validated signature/order while
        // the original native stack is live under admission.
        unsafe { rinzu_callback_frame_word(self, cursor, scalar.map_or(11, |kind| kind as u32)) }
    }
}

#[repr(C)]
struct PreparedFrameEntry {
    control: *mut c_void,
    entry: *const c_void,
}
/// The entry has its validated external signature, with no hidden parameter.
pub struct FrameCallback {
    prepared: PreparedFrameEntry,
    _owner: PhantomData<Rc<()>>,
}
impl FrameCallback {
    /// # Safety
    /// Origin must pin the monitor until close and all admitted receipts settle.
    /// This is a one-time Prepared binding; metadata is copied immediately.
    pub unsafe fn bind_diagnostics(
        &self,
        monitor: &crate::native_diagnostics::ActivityMonitor,
        symbol: &core::ffi::CStr,
        source: crate::failure::SourcePosition,
        owner: u64,
        depth: u64,
    ) {
        // SAFETY: caller retains Origin's board through every admitted receipt.
        unsafe {
            rinzu_callback_entry_bind_diagnostics(
                self.prepared.control,
                monitor.raw_pointer(),
                symbol.as_ptr(),
                source.file,
                source.start_byte,
                source.end_byte,
                owner,
                depth,
            )
        };
    }
    /// # Safety
    /// Queue and captured context/code stay pinned through close and every
    /// admitted receipt. Eligibility must come from the validated signature;
    /// runtime tracks its Main supplant and bounded-native progress exactly.
    pub unsafe fn bind_foreign(
        &self,
        queue: &crate::foreign_queue::ForeignQueue,
        generation: u64,
        eligible: bool,
    ) {
        // SAFETY: caller pins the shared Main queue and immutable signature plan.
        unsafe {
            rinzu_callback_entry_bind_foreign(
                self.prepared.control,
                queue.raw_pointer(),
                generation,
                i32::from(eligible),
            )
        };
    }
    /// # Safety
    /// Pin context through close/settlement. The handler must not unwind and
    /// must decode the exact entry signature before reading native inputs.
    pub unsafe fn prepare(handler: FrameHandler, context: *mut c_void) -> Self {
        Self {
            // SAFETY: context/handler are pinned by the caller's admission contract.
            prepared: unsafe { rinzu_callback_frame_prepare(handler, context) },
            _owner: PhantomData,
        }
    }
    /// Keep admission closed until the Owner's physical native entry.
    ///
    /// # Safety
    /// The same pinned context and exact signature contract as `prepare` applies.
    pub unsafe fn prepare_deferred(handler: FrameHandler, context: *mut c_void) -> Self {
        Self {
            // SAFETY: caller pins context through close and all admitted results.
            prepared: unsafe { rinzu_callback_frame_prepare_deferred(handler, context) },
            _owner: PhantomData,
        }
    }
    pub fn open(&self) {
        // SAFETY: this non-Send wrapper owns a process-retained control.
        unsafe { rinzu_callback_entry_open(self.prepared.control) }
    }
    #[must_use]
    pub fn entry(&self) -> *const c_void {
        self.prepared.entry
    }
    pub fn close(&self) -> bool {
        self.prepared.control.is_null()
            // SAFETY: this Owner retains the control until its explicit retirement.
            || unsafe { rinzu_callback_entry_close(self.prepared.control) != 0 }
    }
    #[must_use]
    pub fn settled(&self) -> bool {
        self.prepared.control.is_null()
            // SAFETY: a non-retired control remains pinned by this Owner.
            || unsafe { rinzu_callback_entry_settled(self.prepared.control) != 0 }
    }
    /// Retire the full control while retaining the entry's permanent NULL slot.
    ///
    /// # Safety
    /// Caller must have a validated whole callback capability proof and physical
    /// return of every template/bridge/pre-admission/result-receipt reader.
    /// No raw control aliases or children may still read the control. Close and
    /// zero in-flight alone are insufficient. This Origin owns the once handle.
    pub unsafe fn retire(&mut self) -> bool {
        // SAFETY: caller closes future capabilities and every physical reader.
        unsafe { rinzu_callback_entry_retire(&mut self.prepared.control) != 0 }
    }
}
impl Drop for FrameCallback {
    fn drop(&mut self) {
        self.close();
    }
}

#[repr(C)]
struct PreparedEntry {
    control: *mut c_void,
    entry: PointerU64Entry,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct CallbackStats {
    pub entries: usize,
    pub blocks: usize,
    pub virtual_bytes: usize,
    pub control_bytes: usize,
    pub live_connections: usize,
    pub in_flight: usize,
    pub invocations: usize,
    pub closed_entries: usize,
    pub tombstone_bytes: usize,
    pub block_metadata_bytes: usize,
    pub live_diagnostics: usize,
    pub diagnostic_bytes: usize,
    pub retired_entries: usize,
    pub retired_slot_bytes: usize,
    pub released_control_bytes: usize,
}

unsafe extern "C" {
    fn rinzu_callback_entry_bind_diagnostics(
        control: *mut c_void,
        board: *mut c_void,
        symbol: *const core::ffi::c_char,
        file: u64,
        start_byte: u32,
        end_byte: u32,
        owner: u64,
        depth: u64,
    );
    fn rinzu_callback_foreign_executing() -> i32;
    fn rinzu_callback_entry_bind_foreign(
        control: *mut c_void,
        queue: *mut c_void,
        generation: u64,
        eligible: i32,
    );
    fn rinzu_callback_foreign_drain_one(queue: *mut c_void) -> i32;
    fn rinzu_callback_frame_prepare(
        handler: FrameHandler,
        context: *mut c_void,
    ) -> PreparedFrameEntry;
    fn rinzu_callback_frame_prepare_deferred(
        handler: FrameHandler,
        context: *mut c_void,
    ) -> PreparedFrameEntry;
    fn rinzu_callback_frame_word(
        frame: *const CallbackFrame,
        cursor: *mut CallbackAbiCursor,
        kind: u32,
    ) -> u64;
    fn rinzu_callback_pointer_u64_prepare(
        handler: PointerU64Handler,
        context: *mut c_void,
    ) -> PreparedEntry;
    fn rinzu_callback_pointer_u64_prepare_deferred(
        handler: PointerU64Handler,
        context: *mut c_void,
    ) -> PreparedEntry;
    fn rinzu_callback_entry_open(control: *mut c_void);
    fn rinzu_callback_entry_close(control: *mut c_void) -> i32;
    fn rinzu_callback_entry_settled(control: *mut c_void) -> i32;
    fn rinzu_callback_entry_retire(owner: *mut *mut c_void) -> i32;
    fn rinzu_callback_entry_stats() -> CallbackStats;
}

/// The preparing Owner executes and closes the synchronous entry. A foreign
/// caller is admitted only through an explicitly bound validated Main queue.
/// Copying the C function pointer does not extend its open admission lifetime.
pub struct PointerU64Callback {
    prepared: PreparedEntry,
    _owner: PhantomData<Rc<()>>,
}

impl PointerU64Callback {
    /// # Safety
    /// Origin pins this monitor through close and all admitted receipts, as in
    /// FrameCallback::bind_diagnostics. Bind exactly once while Prepared.
    pub unsafe fn bind_diagnostics(
        &self,
        monitor: &crate::native_diagnostics::ActivityMonitor,
        symbol: &core::ffi::CStr,
        source: crate::failure::SourcePosition,
        owner: u64,
        depth: u64,
    ) {
        // SAFETY: caller pins the native board; C copies immutable metadata.
        unsafe {
            rinzu_callback_entry_bind_diagnostics(
                self.prepared.control,
                monitor.raw_pointer(),
                symbol.as_ptr(),
                source.file,
                source.start_byte,
                source.end_byte,
                owner,
                depth,
            )
        };
    }
    /// # Safety
    /// Same pinned queue, validated signature and runtime progress contract as
    /// FrameCallback::bind_foreign; pointer data is decoded only by Owner.
    pub unsafe fn bind_foreign(
        &self,
        queue: &crate::foreign_queue::ForeignQueue,
        generation: u64,
        eligible: bool,
    ) {
        // SAFETY: caller pins Main queue/context through all admitted receipts.
        unsafe {
            rinzu_callback_entry_bind_foreign(
                self.prepared.control,
                queue.raw_pointer(),
                generation,
                i32::from(eligible),
            )
        };
    }
    /// Prepare before entering external code. Allocation/image failures are fatal.
    ///
    /// # Safety
    /// `context` must be non-null and stable until admission is closed and every
    /// admitted invocation settles. `handler` must not unwind and must receive
    /// the exact pointer input contract; native copy faults must be fatal.
    pub unsafe fn prepare(handler: PointerU64Handler, context: *mut c_void) -> Self {
        // SAFETY: the caller pins a correctly typed handler/context. The C
        // backend validates the supported template and retains closed controls.
        let prepared = unsafe { rinzu_callback_pointer_u64_prepare(handler, context) };
        Self {
            prepared,
            _owner: PhantomData,
        }
    }

    /// Retain the immutable entry and context with admission still Prepared.
    ///
    /// # Safety
    /// The same pinned context and exact input contract as `prepare` applies.
    pub unsafe fn prepare_deferred(handler: PointerU64Handler, context: *mut c_void) -> Self {
        Self {
            // SAFETY: the caller pins a correctly typed handler and context.
            prepared: unsafe { rinzu_callback_pointer_u64_prepare_deferred(handler, context) },
            _owner: PhantomData,
        }
    }

    pub fn open(&self) {
        // SAFETY: the backend verifies this Owner and the one-time Prepared edge.
        unsafe { rinzu_callback_entry_open(self.prepared.control) }
    }

    #[must_use]
    pub fn entry(&self) -> PointerU64Entry {
        self.prepared.entry
    }

    /// Returns true when the handler connection is released. An admitted nested
    /// invocation can finish after close; no new invocation can be admitted.
    pub fn close(&self) -> bool {
        self.prepared.control.is_null()
            // SAFETY: this Owner retains the control until its explicit retirement.
            || unsafe { rinzu_callback_entry_close(self.prepared.control) != 0 }
    }

    #[must_use]
    pub fn settled(&self) -> bool {
        self.prepared.control.is_null()
            // SAFETY: a non-retired control remains pinned by this Owner.
            || unsafe { rinzu_callback_entry_settled(self.prepared.control) != 0 }
    }
    /// Retire only after closing all capabilities and physical native readers.
    ///
    /// # Safety
    /// The whole-capability/physical-reader/once-owner proof required by
    /// `FrameCallback::retire` applies equally to this exact pointer entry.
    pub unsafe fn retire(&mut self) -> bool {
        // SAFETY: caller closes future capabilities and every physical reader.
        unsafe { rinzu_callback_entry_retire(&mut self.prepared.control) != 0 }
    }
}

impl Drop for PointerU64Callback {
    fn drop(&mut self) {
        self.close();
    }
}

#[must_use]
pub fn stats() -> CallbackStats {
    // SAFETY: no borrowed pointers are returned; pool counters are synchronized.
    unsafe { rinzu_callback_entry_stats() }
}

pub fn drain_foreign_one(queue: &crate::foreign_queue::ForeignQueue) -> bool {
    // SAFETY: !Send Main queue is confined to Owner. C executes only previously
    // admitted/pinned handlers and releases its record reference after result.
    unsafe { rinzu_callback_foreign_drain_one(queue.raw_pointer()) != 0 }
}
/// True only while the common FIFO consumer executes an admitted Owner body.
pub fn is_foreign_execution() -> bool {
    // SAFETY: this function reads only native thread-local admission metadata.
    unsafe { rinzu_callback_foreign_executing() != 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LengthKind {
    Bytes(u64),
    Utf8,
}

struct LengthContext {
    heap: *mut DomainHeap,
    data: *mut ManagedData,
    roots: *mut ManagedRoots,
    kind: LengthKind,
    evidence: LengthEvidence,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LengthEvidence {
    pub prepared_inputs: usize,
    pub body_entries: usize,
    pub copied_bytes: usize,
    pub collections: usize,
    pub moved_objects: usize,
}

/// A closed core length target with real caller-Domain input preparation.
pub struct LengthCallback<'owner> {
    entry: PointerU64Callback,
    context: Box<LengthContext>,
    _owner: PhantomData<(
        &'owner mut DomainHeap,
        &'owner mut ManagedData,
        &'owner mut ManagedRoots,
    )>,
}

impl<'owner> LengthCallback<'owner> {
    pub fn prepare(
        heap: &'owner mut DomainHeap,
        data: &'owner mut ManagedData,
        roots: &'owner mut ManagedRoots,
        kind: LengthKind,
    ) -> Self {
        let mut context = Box::new(LengthContext {
            heap,
            data,
            roots,
            kind,
            evidence: LengthEvidence::default(),
        });
        // SAFETY: exclusive Owner borrows and stable Box live through every
        // synchronous invocation; Drop closes admission before freeing context.
        let entry = unsafe {
            PointerU64Callback::prepare(length_handler, std::ptr::from_mut(&mut *context).cast())
        };
        Self {
            entry,
            context,
            _owner: PhantomData,
        }
    }

    #[must_use]
    pub fn entry(&self) -> PointerU64Entry {
        self.entry.entry()
    }

    #[must_use]
    pub fn evidence(&self) -> LengthEvidence {
        self.context.evidence
    }
}

impl Drop for LengthCallback<'_> {
    fn drop(&mut self) {
        if !self.entry.close() {
            crate::native_fatal::invariant();
        }
        // Keep the pinned context alive until the preceding close has settled.
        let _ = &self.context;
    }
}

unsafe extern "C" fn length_handler(raw: *mut c_void, input: *const u8) -> u64 {
    // SAFETY: the entry gate admitted this physical Owner thread and pinned the
    // context. No Rust Owner reference is held across the external invocation.
    let context = unsafe { &mut *raw.cast::<LengthContext>() };
    // SAFETY: LengthCallback's exclusive borrows remain live on the Owner thread.
    let (heap, data, roots) =
        unsafe { (&mut *context.heap, &mut *context.data, &mut *context.roots) };
    roots.enter_frame();
    let handle = match context.kind {
        // SAFETY: the selected signature determines N; the receiver makes copy
        // faults fatal before exposing any value to the managed body.
        LengthKind::Bytes(length) => unsafe { data.receive_native_bytes(heap, length, input) },
        // SAFETY: native String is a single NUL-terminated pointer; recoverable
        // input failure is unhandled because core length has no fallback.
        LengthKind::Utf8 => unsafe { data.receive_native_string(heap, input) },
    }
    .unwrap_or_else(|_| crate::native_fatal::invariant());
    roots
        .publish([handle])
        .unwrap_or_else(|_| crate::native_fatal::invariant());
    context.evidence.prepared_inputs += 1;
    data.retain_roots(heap, roots.live())
        .unwrap_or_else(|_| crate::native_fatal::invariant());
    let report = heap
        .collect(CollectionKind::Full)
        .unwrap_or_else(|_| crate::native_fatal::invariant());
    context.evidence.collections += 1;
    context.evidence.moved_objects += report.moved_objects;
    let length = match context.kind {
        LengthKind::Bytes(length) => data.byte_length(heap, handle, length),
        LengthKind::Utf8 => data.string_length(heap, handle).map(|length| length as u64),
    }
    .unwrap_or_else(|_| crate::native_fatal::invariant());
    context.evidence.body_entries += 1;
    context.evidence.copied_bytes +=
        usize::try_from(length).unwrap_or_else(|_| crate::native_fatal::invariant());
    roots
        .leave_frame()
        .unwrap_or_else(|_| crate::native_fatal::invariant());
    data.retain_roots(heap, roots.live())
        .unwrap_or_else(|_| crate::native_fatal::invariant());
    let report = heap
        .collect(CollectionKind::Full)
        .unwrap_or_else(|_| crate::native_fatal::invariant());
    context.evidence.collections += 1;
    context.evidence.moved_objects += report.moved_objects;
    length
}
