//! Owner-thread transition stubs and nested native frame publication.

use std::fmt;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

const INACTIVE: u8 = 0;
const MANAGED: u8 = 1;
const NATIVE_SAFE: u8 = 2;
const CALLBACK_ENTRY: u8 = 3;
const CALLBACK_MANAGED: u8 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionState {
    Inactive,
    Managed,
    NativeSafe,
    CallbackEntry,
    CallbackManaged,
}

impl ExecutionState {
    const fn encode(self) -> u8 {
        match self {
            Self::Inactive => INACTIVE,
            Self::Managed => MANAGED,
            Self::NativeSafe => NATIVE_SAFE,
            Self::CallbackEntry => CALLBACK_ENTRY,
            Self::CallbackManaged => CALLBACK_MANAGED,
        }
    }

    fn decode(value: u8) -> Self {
        match value {
            INACTIVE => Self::Inactive,
            MANAGED => Self::Managed,
            NATIVE_SAFE => Self::NativeSafe,
            CALLBACK_ENTRY => Self::CallbackEntry,
            CALLBACK_MANAGED => Self::CallbackManaged,
            _ => unreachable!("execution state is only written by this type"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootPublication {
    pub stack_map_id: u64,
    pub root_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionError {
    WrongThread {
        expected: ThreadId,
        actual: ThreadId,
    },
    InvalidTransition {
        from: ExecutionState,
        to: ExecutionState,
    },
    StaleNativeFrame,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongThread { expected, actual } => {
                write!(
                    formatter,
                    "owner thread is {expected:?}, entered from {actual:?}"
                )
            }
            Self::InvalidTransition { from, to } => {
                write!(formatter, "invalid runtime transition {from:?} -> {to:?}")
            }
            Self::StaleNativeFrame => formatter.write_str("stale native frame token"),
        }
    }
}

impl std::error::Error for TransitionError {}

#[derive(Debug)]
pub struct TransitionMachine {
    owner_thread: ThreadId,
    state: AtomicU8,
    native_generation: AtomicU64,
    published_stack_map: AtomicU64,
    published_roots: AtomicUsize,
    native_frames: Mutex<Vec<NativeFrame>>,
}

#[derive(Debug)]
struct NativeFrame {
    generation: u64,
    return_state: ExecutionState,
    previous_roots: RootPublication,
}

/// Detached from a heap borrow so a native call can synchronously reenter its
/// Owner. The guard stays on that physical thread and restores the outer frame.
pub struct NativeScope {
    machine: Arc<TransitionMachine>,
    generation: u64,
    _owner_only: std::marker::PhantomData<std::rc::Rc<()>>,
}

pub struct CallbackScope {
    machine: Arc<TransitionMachine>,
    generation: u64,
    admitted_after_return: bool,
    _owner_only: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl TransitionMachine {
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner_thread: std::thread::current().id(),
            state: AtomicU8::new(INACTIVE),
            native_generation: AtomicU64::new(0),
            published_stack_map: AtomicU64::new(0),
            published_roots: AtomicUsize::new(0),
            native_frames: Mutex::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn owner_thread(&self) -> ThreadId {
        self.owner_thread
    }

    #[must_use]
    pub fn state(&self) -> ExecutionState {
        ExecutionState::decode(self.state.load(Ordering::Acquire))
    }

    /// Physical native frame nesting on this Owner; parked managed frames do
    /// not contribute to the current activation's diagnostic depth.
    pub fn native_frame_depth(&self) -> Result<usize, TransitionError> {
        self.require_owner()?;
        Ok(self
            .native_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len())
    }

    #[must_use]
    pub fn published_roots(&self) -> Option<RootPublication> {
        let _frames = self
            .native_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        matches!(
            self.state(),
            ExecutionState::NativeSafe
                | ExecutionState::CallbackEntry
                | ExecutionState::CallbackManaged
        )
        .then(|| RootPublication {
            stack_map_id: self.published_stack_map.load(Ordering::Acquire),
            root_count: self.published_roots.load(Ordering::Acquire),
        })
    }

    pub fn activate_managed(&self) -> Result<(), TransitionError> {
        self.require_owner()?;
        self.transition(ExecutionState::Inactive, ExecutionState::Managed)
    }

    pub fn deactivate_managed(&self) -> Result<(), TransitionError> {
        self.require_owner()?;
        self.transition(ExecutionState::Managed, ExecutionState::Inactive)
    }

    pub(crate) fn enter_native(&self, roots: RootPublication) -> Result<u64, TransitionError> {
        self.require_owner()?;
        let return_state = self.state();
        if !matches!(
            return_state,
            ExecutionState::Managed | ExecutionState::CallbackManaged
        ) {
            return Err(TransitionError::InvalidTransition {
                from: return_state,
                to: ExecutionState::NativeSafe,
            });
        }
        let mut frames = self
            .native_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        frames
            .try_reserve(1)
            .unwrap_or_else(|_| crate::allocation::fatal());
        let generation = self
            .native_generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                value.checked_add(1)
            })
            .unwrap_or_else(|_| {
                // SAFETY: generation exhaustion must not reconnect a stale frame.
                unsafe { libc::_exit(86) }
            })
            + 1;
        let previous_roots = RootPublication {
            stack_map_id: self.published_stack_map.load(Ordering::Acquire),
            root_count: self.published_roots.load(Ordering::Acquire),
        };
        self.published_stack_map
            .store(roots.stack_map_id, Ordering::Release);
        self.published_roots
            .store(roots.root_count, Ordering::Release);
        if let Err(error) = self.transition(return_state, ExecutionState::NativeSafe) {
            self.published_stack_map
                .store(previous_roots.stack_map_id, Ordering::Release);
            self.published_roots
                .store(previous_roots.root_count, Ordering::Release);
            return Err(error);
        }
        frames.push(NativeFrame {
            generation,
            return_state,
            previous_roots,
        });
        Ok(generation)
    }

    pub(crate) fn leave_native(&self, generation: u64) -> Result<(), TransitionError> {
        self.require_owner()?;
        let mut frames = self
            .native_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(frame) = frames.last().filter(|frame| frame.generation == generation) else {
            return Err(TransitionError::StaleNativeFrame);
        };
        self.transition(ExecutionState::NativeSafe, frame.return_state)?;
        self.published_stack_map
            .store(frame.previous_roots.stack_map_id, Ordering::Release);
        self.published_roots
            .store(frame.previous_roots.root_count, Ordering::Release);
        frames.pop();
        Ok(())
    }

    pub(crate) fn enter_callback(&self, generation: u64) -> Result<(), TransitionError> {
        self.require_owner()?;
        self.require_native_frame(generation)?;
        self.transition(ExecutionState::NativeSafe, ExecutionState::CallbackEntry)
    }

    pub(crate) fn leave_callback(&self, generation: u64) -> Result<(), TransitionError> {
        self.require_owner()?;
        self.require_native_frame(generation)?;
        self.transition(ExecutionState::CallbackEntry, ExecutionState::NativeSafe)
    }

    pub(crate) fn activate_callback_managed(&self) -> Result<(), TransitionError> {
        self.require_owner()?;
        self.transition(
            ExecutionState::CallbackEntry,
            ExecutionState::CallbackManaged,
        )
    }

    pub(crate) fn deactivate_callback_managed(&self) -> Result<(), TransitionError> {
        self.require_owner()?;
        self.transition(
            ExecutionState::CallbackManaged,
            ExecutionState::CallbackEntry,
        )
    }

    fn transition(&self, from: ExecutionState, to: ExecutionState) -> Result<(), TransitionError> {
        self.state
            .compare_exchange(
                from.encode(),
                to.encode(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|actual| TransitionError::InvalidTransition {
                from: ExecutionState::decode(actual),
                to,
            })
    }

    fn require_owner(&self) -> Result<(), TransitionError> {
        let actual = std::thread::current().id();
        if actual == self.owner_thread {
            Ok(())
        } else {
            Err(TransitionError::WrongThread {
                expected: self.owner_thread,
                actual,
            })
        }
    }

    fn require_native_frame(&self, generation: u64) -> Result<(), TransitionError> {
        let frames = self
            .native_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if frames
            .last()
            .is_some_and(|frame| frame.generation == generation)
        {
            Ok(())
        } else {
            Err(TransitionError::StaleNativeFrame)
        }
    }

    pub fn native_scope(
        self: &Arc<Self>,
        roots: RootPublication,
    ) -> Result<NativeScope, TransitionError> {
        let generation = self.enter_native(roots)?;
        Ok(NativeScope {
            machine: Arc::clone(self),
            generation,
            _owner_only: std::marker::PhantomData,
        })
    }

    pub fn callback_scope(self: &Arc<Self>) -> Result<CallbackScope, TransitionError> {
        self.require_owner()?;
        let generation = self
            .native_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last()
            .ok_or(TransitionError::StaleNativeFrame)?
            .generation;
        self.enter_callback(generation)?;
        if let Err(error) = self.activate_callback_managed() {
            let _ = self.leave_callback(generation);
            return Err(error);
        }
        Ok(CallbackScope {
            machine: Arc::clone(self),
            generation,
            admitted_after_return: false,
            _owner_only: std::marker::PhantomData,
        })
    }

    /// # Safety
    /// A common callback gate must already have granted and pinned this Owner
    /// invocation. Only accepted foreign work may execute after native return.
    pub unsafe fn admitted_callback_scope(
        self: &Arc<Self>,
    ) -> Result<CallbackScope, TransitionError> {
        self.require_owner()?;
        if self.state() == ExecutionState::NativeSafe {
            return self.callback_scope();
        }
        if !self
            .native_frames
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty()
        {
            return Err(TransitionError::StaleNativeFrame);
        }
        self.transition(ExecutionState::Managed, ExecutionState::CallbackEntry)?;
        self.activate_callback_managed()?;
        Ok(CallbackScope {
            machine: Arc::clone(self),
            generation: 0,
            admitted_after_return: true,
            _owner_only: std::marker::PhantomData,
        })
    }
}

fn require_restoration(result: Result<(), TransitionError>) {
    if result.is_err() {
        // SAFETY: corrupted transitions cannot unwind through a native frame.
        unsafe { libc::_exit(86) }
    }
}
impl Drop for NativeScope {
    fn drop(&mut self) {
        require_restoration(self.machine.leave_native(self.generation));
    }
}
impl Drop for CallbackScope {
    fn drop(&mut self) {
        require_restoration(self.machine.deactivate_callback_managed());
        if self.admitted_after_return {
            require_restoration(
                self.machine
                    .transition(ExecutionState::CallbackEntry, ExecutionState::Managed),
            );
        } else {
            require_restoration(self.machine.leave_callback(self.generation));
        }
    }
}

impl Default for TransitionMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn wrong_thread_and_illegal_edges_are_typed_failures() {
        let machine = Arc::new(TransitionMachine::new());
        assert!(matches!(
            machine.leave_native(1),
            Err(TransitionError::StaleNativeFrame)
        ));
        let other = Arc::clone(&machine);
        let error = std::thread::spawn(move || other.activate_managed().unwrap_err())
            .join()
            .unwrap();
        assert!(matches!(error, TransitionError::WrongThread { .. }));
    }

    #[test]
    fn admitted_after_return_callbacks_keep_owner_gc_and_nested_native_restoration() {
        use crate::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
        let machine = Arc::new(TransitionMachine::new());
        machine.activate_managed().unwrap();
        let mut heap = DomainHeap::new(71);
        let value = heap
            .allocate(7, &[HeapValue::Integer(42)], FinalizerPolicy::None)
            .unwrap();
        let root = heap.create_root(Some(value)).unwrap();
        let native = machine
            .native_scope(RootPublication {
                stack_map_id: 7,
                root_count: 1,
            })
            .unwrap();
        drop(native);
        assert!(machine.callback_scope().is_err());
        // SAFETY: the model retains its granted admission and independent root
        // through Owner completion, matching the actual common-entry fixture.
        let callback = unsafe { machine.admitted_callback_scope() }.unwrap();
        assert_eq!(machine.state(), ExecutionState::CallbackManaged);
        heap.collect(CollectionKind::Full).unwrap();
        let nested = machine
            .native_scope(RootPublication {
                stack_map_id: 8,
                root_count: 1,
            })
            .unwrap();
        let reentry = machine.callback_scope().unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        drop(reentry);
        drop(nested);
        drop(callback);
        assert_eq!(machine.state(), ExecutionState::Managed);
        assert!(machine.published_roots().is_none());
        assert_eq!(
            heap.root_target(root).unwrap().unwrap().epoch(),
            value.epoch() + 2
        );
        heap.release_root(root).unwrap();
        heap.collect(CollectionKind::Full).unwrap();
        assert_eq!(heap.object_count(), 0);
        machine.deactivate_managed().unwrap();
    }

    #[test]
    fn nested_native_frames_restore_generation_and_outer_publication_after_gc() {
        use crate::heap::{CollectionKind, DomainHeap, FinalizerPolicy, HeapValue};
        fn recurse(machine: &Arc<TransitionMachine>, heap: &mut DomainHeap, depth: usize) {
            let publication = RootPublication {
                stack_map_id: depth as u64 + 1,
                root_count: depth + 2,
            };
            let native = machine.native_scope(publication).unwrap();
            assert_eq!(machine.state(), ExecutionState::NativeSafe);
            assert_eq!(machine.published_roots(), Some(publication));
            let callback = machine.callback_scope().unwrap();
            assert_eq!(machine.state(), ExecutionState::CallbackManaged);
            heap.collect(CollectionKind::Full).unwrap();
            if depth < 63 {
                recurse(machine, heap, depth + 1);
                assert_eq!(machine.state(), ExecutionState::CallbackManaged);
                assert_eq!(machine.published_roots(), Some(publication));
            }
            drop(callback);
            assert_eq!(machine.state(), ExecutionState::NativeSafe);
            assert_eq!(machine.published_roots(), Some(publication));
            // A stale/outer generation must not remove any frame or publication.
            assert_eq!(
                machine.leave_native(native.generation + 1),
                Err(TransitionError::StaleNativeFrame)
            );
            assert_eq!(machine.published_roots(), Some(publication));
            drop(native);
        }
        fn owner() {
            let machine = Arc::new(TransitionMachine::new());
            machine.activate_managed().unwrap();
            let mut heap = DomainHeap::new(41);
            let object = heap
                .allocate(7, &[HeapValue::Integer(42)], FinalizerPolicy::None)
                .unwrap();
            let root = heap.create_root(Some(object)).unwrap();
            recurse(&machine, &mut heap, 0);
            assert_eq!(
                heap.root_target(root).unwrap().unwrap().epoch(),
                object.epoch() + 64
            );
            assert_eq!(machine.state(), ExecutionState::Managed);
            assert!(machine.published_roots().is_none());
            assert!(machine.native_frames.lock().unwrap().is_empty());
            machine.deactivate_managed().unwrap();
        }
        owner();
        let workers: Vec<_> = (0..8).map(|_| std::thread::spawn(owner)).collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }
}
