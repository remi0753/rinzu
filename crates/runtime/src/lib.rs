//! Minimal standalone runtime and the versioned C boundary established by I0.

pub mod allocation;
pub mod blocking;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod callback_entry;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod callback_return;
pub mod child_scope;
pub mod code;
pub mod data;
pub mod diagnostic;
pub mod dispatch;
pub mod error_groups;
pub mod error_trace;
pub mod error_value;
pub mod failure;
pub mod failure_transport;
#[cfg(unix)]
pub mod foreign_queue;
pub mod gc;
pub mod heap;
pub mod host_loop;
pub mod key;
pub mod native_completion;
#[cfg(unix)]
pub mod native_diagnostics;
pub mod native_fatal;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod owner_dispatcher;
pub mod reactor;
#[cfg(unix)]
pub mod ready_work;
pub mod resource_accounting;
pub mod resource_report;
pub mod resources;
pub mod scalar;
pub mod scheduler;
pub mod spawn;
pub mod standalone;
pub mod stdio;
pub mod supplant;
pub mod transition;
pub mod transport;
pub mod value;
pub mod worker;

use core::ffi::c_void;
use core::ptr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use rinzu_abi::{
    ABI_VERSION, AbiDescriptor, HostLoopProfileV1, HostWorkCallback, RuntimeOptions,
    STATUS_ABI_MISMATCH, STATUS_INVALID_ARGUMENT, STATUS_OK, STATUS_PANIC, STATUS_QUEUE_FULL,
    STATUS_UNSUPPORTED, STATUS_WRONG_THREAD, Status, WakeCallback, is_compatible,
};

use crate::host_loop::{
    CAbiHostLoopAdapter, HostActivation, HostLoopController, HostLoopError,
    STANDARD_HOST_QUEUE_CAPACITY,
};

pub const V1_TARGET: &str = "aarch64-apple-darwin";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedTarget {
    pub expected: &'static str,
    pub actual: &'static str,
}

impl core::fmt::Display for UnsupportedTarget {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "Rinzu v1 requires {}; current target is {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for UnsupportedTarget {}

#[must_use]
pub const fn current_target() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows"
    } else {
        "unsupported-unknown-target"
    }
}

pub fn validate_v1_target() -> Result<(), UnsupportedTarget> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Ok(())
    } else {
        Err(UnsupportedTarget {
            expected: V1_TARGET,
            actual: current_target(),
        })
    }
}

#[derive(Debug)]
pub struct EmptyRun {
    pub target: &'static str,
}

pub fn run_empty_standalone() -> Result<EmptyRun, UnsupportedTarget> {
    validate_v1_target()?;
    Ok(EmptyRun {
        target: current_target(),
    })
}

#[derive(Clone, Copy)]
struct WakeTarget {
    callback: Option<WakeCallback>,
    context: *mut c_void,
}

// SAFETY: the C ABI requires callback context storage to be safe for calls from
// any thread for the whole registration lifetime. Runtime code never dereferences it.
unsafe impl Send for WakeTarget {}

#[repr(C)]
pub struct Runtime {
    wake: Mutex<WakeTarget>,
    transitions: transition::TransitionMachine,
}

#[repr(C)]
pub struct HostSession {
    controller: Arc<HostLoopController>,
    activation: Mutex<Option<HostActivation>>,
}

#[derive(Clone, Copy)]
struct CWorkTarget {
    callback: HostWorkCallback,
    context: *mut c_void,
}

// SAFETY: the public C ABI requires an enqueued callback and context to remain
// valid for cross-thread delivery until invocation or Host Session leave.
unsafe impl Send for CWorkTarget {}

impl CWorkTarget {
    fn invoke(self) {
        // SAFETY: validity and cross-thread access are the enqueue caller's ABI
        // obligations; HostWork owns this borrowed pair until this invocation.
        unsafe { (self.callback)(self.context) };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    WrongThread {
        expected: ThreadId,
        actual: ThreadId,
    },
    ManagedActivationBusy,
}

impl core::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongThread { expected, actual } => write!(
                formatter,
                "Main Domain requires process-start thread {expected:?}, entered from {actual:?}"
            ),
            Self::ManagedActivationBusy => {
                write!(formatter, "Main Domain already has a managed activation")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

pub struct ManagedActivation<'a> {
    runtime: &'a Runtime,
}

pub struct NativeSafe<'a> {
    runtime: &'a Runtime,
    generation: u64,
}

pub struct CallbackEntry<'a> {
    runtime: &'a Runtime,
    generation: u64,
}

pub struct CallbackManaged<'a> {
    runtime: &'a Runtime,
}

impl Drop for ManagedActivation<'_> {
    fn drop(&mut self) {
        self.runtime
            .transitions
            .deactivate_managed()
            .expect("ManagedActivation lifetime prevents an open native transition");
    }
}

impl ManagedActivation<'_> {
    pub fn enter_native_safe(
        &self,
        roots: transition::RootPublication,
    ) -> Result<NativeSafe<'_>, transition::TransitionError> {
        let generation = self.runtime.transitions.enter_native(roots)?;
        Ok(NativeSafe {
            runtime: self.runtime,
            generation,
        })
    }
}

impl NativeSafe<'_> {
    pub fn enter_callback(&self) -> Result<CallbackEntry<'_>, transition::TransitionError> {
        self.runtime.transitions.enter_callback(self.generation)?;
        Ok(CallbackEntry {
            runtime: self.runtime,
            generation: self.generation,
        })
    }
}

impl Drop for NativeSafe<'_> {
    fn drop(&mut self) {
        self.runtime
            .transitions
            .leave_native(self.generation)
            .expect("NativeSafe lifetime prevents an open callback transition");
    }
}

impl CallbackEntry<'_> {
    pub fn enter_managed(&self) -> Result<CallbackManaged<'_>, transition::TransitionError> {
        self.runtime.transitions.activate_callback_managed()?;
        Ok(CallbackManaged {
            runtime: self.runtime,
        })
    }
}

impl Drop for CallbackEntry<'_> {
    fn drop(&mut self) {
        self.runtime
            .transitions
            .leave_callback(self.generation)
            .expect("CallbackEntry lifetime prevents an open managed callback");
    }
}

impl Drop for CallbackManaged<'_> {
    fn drop(&mut self) {
        self.runtime
            .transitions
            .deactivate_callback_managed()
            .expect("callback managed state must return to CallbackEntry");
    }
}

impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            wake: Mutex::new(WakeTarget {
                callback: None,
                context: ptr::null_mut(),
            }),
            transitions: transition::TransitionMachine::new(),
        }
    }

    #[must_use]
    pub fn owner_thread(&self) -> ThreadId {
        self.transitions.owner_thread()
    }

    #[must_use]
    pub fn execution_state(&self) -> transition::ExecutionState {
        self.transitions.state()
    }

    pub fn enter_managed(&self) -> Result<ManagedActivation<'_>, RuntimeError> {
        self.transitions
            .activate_managed()
            .map_err(|error| match error {
                transition::TransitionError::WrongThread { expected, actual } => {
                    RuntimeError::WrongThread { expected, actual }
                }
                transition::TransitionError::InvalidTransition { .. }
                | transition::TransitionError::StaleNativeFrame => {
                    RuntimeError::ManagedActivationBusy
                }
            })?;
        Ok(ManagedActivation { runtime: self })
    }

    pub fn request_wake(&self) {
        let target = *self
            .wake
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(callback) = target.callback {
            // SAFETY: callback and context validity are guaranteed by the C ABI
            // registration contract for this runtime's lifetime.
            unsafe { callback(target.context) };
        }
    }
}

#[repr(C)]
pub struct StableHandle {
    references: AtomicUsize,
    target: std::sync::Arc<heap::StableTarget>,
}

impl StableHandle {
    fn from_target(target: std::sync::Arc<heap::StableTarget>) -> *mut Self {
        Box::into_raw(Box::new(Self {
            references: AtomicUsize::new(1),
            target,
        }))
    }

    #[cfg(test)]
    fn new_for_test() -> *mut Self {
        Self::from_target(heap::StableTarget::empty_for_abi_test())
    }
}

fn ffi_status(operation: impl FnOnce() -> Status) -> Status {
    catch_unwind(AssertUnwindSafe(operation)).unwrap_or(STATUS_PANIC)
}

#[unsafe(no_mangle)]
pub extern "C" fn rinzu_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
/// Writes the runtime ABI descriptor to caller-owned storage.
///
/// # Safety
///
/// `out_descriptor` must be null or point to writable storage for one descriptor.
pub unsafe extern "C" fn rinzu_abi_query(out_descriptor: *mut AbiDescriptor) -> Status {
    ffi_status(|| {
        if out_descriptor.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: non-null writable output storage is required by the C ABI.
        unsafe { out_descriptor.write(AbiDescriptor::default()) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Initializes caller-owned runtime options with v1 defaults.
///
/// # Safety
///
/// `out_options` must be null or point to writable storage for one options value.
pub unsafe extern "C" fn rinzu_runtime_options_init(out_options: *mut RuntimeOptions) -> Status {
    ffi_status(|| {
        if out_options.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: non-null writable output storage is required by the C ABI.
        unsafe { out_options.write(RuntimeOptions::default()) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Creates one owned runtime reference.
///
/// # Safety
///
/// `options` must be null or point to a readable options prefix, and
/// `out_runtime` must be null or writable for one pointer.
pub unsafe extern "C" fn rinzu_runtime_create(
    options: *const RuntimeOptions,
    out_runtime: *mut *mut Runtime,
) -> Status {
    ffi_status(|| {
        if out_runtime.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: non-null writable output storage is required by the C ABI.
        unsafe { out_runtime.write(ptr::null_mut()) };

        if !options.is_null() {
            // SAFETY: a non-null options pointer must identify a readable ABI struct.
            let options = unsafe { &*options };
            if options.struct_size < size_of::<RuntimeOptions>() as u32 {
                return STATUS_INVALID_ARGUMENT;
            }
            if !is_compatible(options.abi_version) {
                return STATUS_ABI_MISMATCH;
            }
        }

        let runtime = Box::into_raw(Box::new(Runtime::new()));
        // SAFETY: out_runtime was validated above and runtime is uniquely owned.
        unsafe { out_runtime.write(runtime) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Consumes one owned runtime reference.
///
/// # Safety
///
/// `runtime` must be null or the unique live pointer returned by create.
pub unsafe extern "C" fn rinzu_runtime_destroy(runtime: *mut Runtime) {
    if runtime.is_null() {
        return;
    }
    // SAFETY: the C ABI transfers exactly one owned runtime reference here.
    unsafe { drop(Box::from_raw(runtime)) };
}

#[unsafe(no_mangle)]
/// Replaces the runtime wake callback and borrowed context.
///
/// # Safety
///
/// `runtime` must be live. A non-null callback and its context must satisfy the
/// lifetime and cross-thread rules in the public C header.
pub unsafe extern "C" fn rinzu_runtime_set_wake_callback(
    runtime: *mut Runtime,
    callback: Option<WakeCallback>,
    context: *mut c_void,
) -> Status {
    ffi_status(|| {
        // SAFETY: a non-null runtime pointer is owned by the caller and remains
        // live for the duration of this call.
        let Some(runtime) = (unsafe { runtime.as_ref() }) else {
            return STATUS_INVALID_ARGUMENT;
        };
        let mut target = runtime
            .wake
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *target = WakeTarget { callback, context };
        STATUS_OK
    })
}

fn host_error_status(error: &HostLoopError) -> Status {
    match error {
        HostLoopError::WrongThread { .. } => STATUS_WRONG_THREAD,
        HostLoopError::QueueFull => STATUS_QUEUE_FULL,
        HostLoopError::InvalidProfile(_) => STATUS_INVALID_ARGUMENT,
        HostLoopError::AlreadyEntered
        | HostLoopError::GenerationExhausted
        | HostLoopError::PrepareFailed(_)
        | HostLoopError::QueueClosed
        | HostLoopError::StaleGeneration => STATUS_UNSUPPORTED,
    }
}

#[unsafe(no_mangle)]
/// Creates an owner-thread Host Session around a v1 profile.
///
/// # Safety
///
/// `runtime` must be live, `profile` must be readable and its borrowed state
/// must outlive the session, and `out_session` must be writable.
pub unsafe extern "C" fn rinzu_host_session_create(
    runtime: *mut Runtime,
    profile: *const HostLoopProfileV1,
    out_session: *mut *mut HostSession,
) -> Status {
    ffi_status(|| {
        if runtime.is_null() || profile.is_null() || out_session.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: validated output storage and readable profile are required by
        // the function contract.
        unsafe { out_session.write(ptr::null_mut()) };
        // SAFETY: profile is non-null and readable for this call.
        let profile = unsafe { *profile };
        if !is_compatible(profile.abi_version) {
            return STATUS_ABI_MISMATCH;
        }
        let adapter = match CAbiHostLoopAdapter::new(profile) {
            Ok(adapter) => adapter,
            Err(error) => return host_error_status(&error),
        };
        let turn_budget = adapter.turn_budget();
        let controller =
            HostLoopController::new(Arc::new(adapter), STANDARD_HOST_QUEUE_CAPACITY, turn_budget);
        let session = Box::into_raw(Box::new(HostSession {
            controller,
            activation: Mutex::new(None),
        }));
        // SAFETY: output storage was validated and receives one owned session.
        unsafe { out_session.write(session) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Enters a Host Session on its owner thread.
///
/// # Safety
///
/// `session` must be live and `out_generation` must be writable.
pub unsafe extern "C" fn rinzu_host_session_enter(
    session: *mut HostSession,
    out_generation: *mut u64,
) -> Status {
    ffi_status(|| {
        // SAFETY: a non-null session pointer is live for this call.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return STATUS_INVALID_ARGUMENT;
        };
        if out_generation.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        let mut activation = session
            .activation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if activation.is_some() {
            return STATUS_UNSUPPORTED;
        }
        let entered = match session.controller.enter() {
            Ok(entered) => entered,
            Err(error) => return host_error_status(&error),
        };
        let generation = entered.generation();
        *activation = Some(entered);
        // SAFETY: output storage was validated above.
        unsafe { out_generation.write(generation) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Enqueues one callback for bounded owner-thread delivery.
///
/// # Safety
///
/// `session` must be live. The callback and context must remain valid until
/// callback invocation or until owner-thread leave returns.
pub unsafe extern "C" fn rinzu_host_session_enqueue(
    session: *mut HostSession,
    callback: Option<HostWorkCallback>,
    context: *mut c_void,
) -> Status {
    ffi_status(|| {
        // SAFETY: a non-null session pointer is live for this call.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return STATUS_INVALID_ARGUMENT;
        };
        let Some(callback) = callback else {
            return STATUS_INVALID_ARGUMENT;
        };
        let target = CWorkTarget { callback, context };
        match session.controller.enqueue(move || target.invoke()) {
            Ok(_) => STATUS_OK,
            Err(error) => host_error_status(&error),
        }
    })
}

#[unsafe(no_mangle)]
/// Leaves a Host Session after all in-flight host posts have returned.
///
/// # Safety
///
/// `session` must be live and this call must run on the creating owner thread.
pub unsafe extern "C" fn rinzu_host_session_leave(session: *mut HostSession) -> Status {
    ffi_status(|| {
        // SAFETY: a non-null session pointer is live for this call.
        let Some(session) = (unsafe { session.as_ref() }) else {
            return STATUS_INVALID_ARGUMENT;
        };
        if thread::current().id() != session.controller.owner_thread() {
            return STATUS_WRONG_THREAD;
        }
        let activation = session
            .activation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let Some(activation) = activation else {
            return STATUS_UNSUPPORTED;
        };
        match activation.leave() {
            Ok(_) => STATUS_OK,
            Err(error) => host_error_status(&error),
        }
    })
}

#[unsafe(no_mangle)]
/// Consumes one owner-thread Host Session reference.
///
/// # Safety
///
/// `session` must be null or the unique live pointer returned by create.
pub unsafe extern "C" fn rinzu_host_session_destroy(session: *mut HostSession) -> Status {
    ffi_status(|| {
        if session.is_null() {
            return STATUS_OK;
        }
        // SAFETY: session is non-null and live for this ownership check.
        let session_ref = unsafe { &*session };
        if thread::current().id() != session_ref.controller.owner_thread() {
            return STATUS_WRONG_THREAD;
        }
        let activation = session_ref
            .activation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(activation) = activation
            && let Err(error) = activation.leave()
        {
            return host_error_status(&error);
        }
        // SAFETY: the caller transfers its unique owned session reference.
        unsafe { drop(Box::from_raw(session)) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Creates one additional owned reference to a stable handle.
///
/// # Safety
///
/// `handle` must denote a live owned reference and `out_handle` must be writable.
pub unsafe extern "C" fn rinzu_handle_retain(
    handle: *const StableHandle,
    out_handle: *mut *mut StableHandle,
) -> Status {
    ffi_status(|| {
        if handle.is_null() || out_handle.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: both pointers are live according to the stable-handle contract.
        unsafe { out_handle.write(ptr::null_mut()) };
        // SAFETY: handle is non-null and denotes an owned live reference.
        let handle_ref = unsafe { &*handle };
        let retained =
            handle_ref
                .references
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    current.checked_add(1).filter(|_| current != 0)
                });
        if retained.is_err() {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: the increment above created one owned reference for the output.
        unsafe { out_handle.write(handle.cast_mut()) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
/// Consumes one owned stable-handle reference.
///
/// # Safety
///
/// `handle` must be null or denote exactly one live owned reference.
pub unsafe extern "C" fn rinzu_handle_release(handle: *mut StableHandle) {
    if handle.is_null() {
        return;
    }
    // SAFETY: a non-null pointer passed here represents one owned live reference.
    let handle_ref = unsafe { &*handle };
    if handle_ref.references.fetch_sub(1, Ordering::AcqRel) == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        // SAFETY: this release observed and consumed the final reference.
        unsafe { drop(Box::from_raw(handle)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    unsafe extern "C" fn count_wake(context: *mut c_void) {
        // SAFETY: the test registers an AtomicUsize with a longer lifetime.
        let counter = unsafe { &*(context.cast::<AtomicUsize>()) };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    #[test]
    fn runtime_abi_query_create_callback_and_destroy() {
        let mut descriptor = AbiDescriptor::default();
        // SAFETY: all pointers refer to writable local storage or live runtime state.
        unsafe {
            assert_eq!(rinzu_abi_query(&mut descriptor), STATUS_OK);
            assert_eq!(descriptor.abi_version, ABI_VERSION);

            let options = RuntimeOptions::default();
            let mut runtime = ptr::null_mut();
            assert_eq!(rinzu_runtime_create(&options, &mut runtime), STATUS_OK);

            let wakes = AtomicUsize::new(0);
            assert_eq!(
                rinzu_runtime_set_wake_callback(
                    runtime,
                    Some(count_wake),
                    ptr::from_ref(&wakes).cast_mut().cast(),
                ),
                STATUS_OK
            );
            (*runtime).request_wake();
            assert_eq!(wakes.load(Ordering::Relaxed), 1);
            rinzu_runtime_destroy(runtime);
        }
    }

    #[test]
    fn managed_activation_is_owner_thread_only_and_non_reentrant() {
        let runtime = std::sync::Arc::new(Runtime::new());
        let activation = runtime.enter_managed().unwrap();
        assert!(matches!(
            runtime.enter_managed(),
            Err(RuntimeError::ManagedActivationBusy)
        ));
        drop(activation);
        assert!(runtime.enter_managed().is_ok());

        let other = std::sync::Arc::clone(&runtime);
        let error = std::thread::spawn(move || other.enter_managed().err().unwrap())
            .join()
            .unwrap();
        assert!(matches!(error, RuntimeError::WrongThread { .. }));
    }

    #[test]
    fn native_and_owner_callback_states_follow_the_legal_path() {
        use crate::transition::{ExecutionState, RootPublication};

        let runtime = Runtime::new();
        let activation = runtime.enter_managed().unwrap();
        assert_eq!(runtime.execution_state(), ExecutionState::Managed);
        {
            let native = activation
                .enter_native_safe(RootPublication {
                    stack_map_id: 17,
                    root_count: 3,
                })
                .unwrap();
            assert_eq!(runtime.execution_state(), ExecutionState::NativeSafe);
            assert_eq!(runtime.transitions.published_roots().unwrap().root_count, 3);
            {
                let callback = native.enter_callback().unwrap();
                assert_eq!(runtime.execution_state(), ExecutionState::CallbackEntry);
                {
                    let _managed = callback.enter_managed().unwrap();
                    assert_eq!(runtime.execution_state(), ExecutionState::CallbackManaged);
                }
                assert_eq!(runtime.execution_state(), ExecutionState::CallbackEntry);
            }
            assert_eq!(runtime.execution_state(), ExecutionState::NativeSafe);
        }
        assert_eq!(runtime.execution_state(), ExecutionState::Managed);
        assert!(runtime.transitions.published_roots().is_none());
        drop(activation);
        assert_eq!(runtime.execution_state(), ExecutionState::Inactive);
    }

    #[test]
    fn owner_sync_reentry_collects_after_a_long_native_safe_interval() {
        use crate::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
        use crate::transition::{ExecutionState, RootPublication};
        use std::sync::Arc;

        let runtime = Arc::new(Runtime::new());
        let mut heap = DomainHeap::new(41);
        let object = heap
            .allocate(7, &[HeapValue::Integer(42)], FinalizerPolicy::None)
            .unwrap();
        let root = heap.create_root(Some(object)).unwrap();
        let stable = heap.create_stable_handle(object).unwrap();

        let activation = runtime.enter_managed().unwrap();
        let native = activation
            .enter_native_safe(RootPublication {
                stack_map_id: 99,
                root_count: 2,
            })
            .unwrap();
        let observer_runtime = Arc::clone(&runtime);
        let observer = std::thread::spawn(move || {
            for _ in 0..50_000 {
                assert_eq!(
                    observer_runtime.execution_state(),
                    ExecutionState::NativeSafe
                );
            }
        });
        observer.join().unwrap();

        let callback = native.enter_callback().unwrap();
        let callback_managed = callback.enter_managed().unwrap();
        let before = heap.root_target(root).unwrap().unwrap();
        let report = heap.collect(CollectionKind::Full).unwrap();
        let after = heap.root_target(root).unwrap().unwrap();
        // SAFETY: the stable handle is live and belongs to this heap.
        let stable_after = unsafe { heap.resolve_stable_handle(stable) }
            .unwrap()
            .unwrap();
        assert_eq!(report.moved_objects, 1);
        assert_ne!(before.epoch(), after.epoch());
        assert_eq!(after, stable_after);

        drop(callback_managed);
        drop(callback);
        drop(native);
        drop(activation);
        assert_eq!(runtime.execution_state(), ExecutionState::Inactive);
        // SAFETY: this test owns the sole stable-handle reference.
        unsafe { rinzu_handle_release(stable) };
    }

    #[test]
    fn stable_handle_retain_and_release_balance() {
        let handle = StableHandle::new_for_test();
        let mut retained = ptr::null_mut();
        // SAFETY: the test owns one handle and releases both resulting references.
        unsafe {
            assert_eq!(rinzu_handle_retain(handle, &mut retained), STATUS_OK);
            assert_eq!(retained, handle);
            assert_eq!((*handle).references.load(Ordering::Acquire), 2);
            rinzu_handle_release(retained);
            assert_eq!((*handle).references.load(Ordering::Acquire), 1);
            rinzu_handle_release(handle);
        }
    }
}
