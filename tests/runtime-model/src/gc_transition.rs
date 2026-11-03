//! Owner-thread stop-the-Domain GC and native transition prototype.

use std::fmt::{self, Display, Formatter};
use std::thread::{self, ThreadId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadState {
    Managed,
    NativeSafe,
    CallbackEntry,
    CallbackManaged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GcError {
    WrongThread,
    InvalidTransition,
    CollectionDeferred,
    InvalidHandle,
    UnknownObject,
}

impl Display for GcError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for GcError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StableHandle {
    slot: usize,
    generation: u32,
}

#[derive(Clone, Copy, Debug)]
struct Object {
    id: u64,
    location: u64,
    value: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct HandleSlot {
    generation: u32,
    target_id: Option<u64>,
    target_location: u64,
}

/// A small movable heap whose moving/update phase may run only on its owner.
#[derive(Debug)]
pub struct DomainHeap {
    owner: ThreadId,
    state: ThreadState,
    roots_published: bool,
    objects: Vec<Object>,
    handles: Vec<HandleSlot>,
    next_object_id: u64,
    next_location: u64,
    collection_count: u64,
    allocation_threshold: usize,
}

// Mutex poisoning in this executable prototype is an unrecoverable test
// failure. Public operations report semantic transition failures explicitly.
#[allow(clippy::missing_errors_doc)]
impl DomainHeap {
    #[must_use]
    pub fn new(allocation_threshold: usize) -> Self {
        Self {
            owner: thread::current().id(),
            state: ThreadState::Managed,
            roots_published: false,
            objects: Vec::new(),
            handles: Vec::new(),
            next_object_id: 1,
            next_location: 1_000,
            collection_count: 0,
            allocation_threshold,
        }
    }

    #[must_use]
    pub const fn state(&self) -> ThreadState {
        self.state
    }

    #[must_use]
    pub const fn collection_count(&self) -> u64 {
        self.collection_count
    }

    pub fn allocate(&mut self, value: u64) -> Result<u64, GcError> {
        self.require_owner()?;
        if !matches!(
            self.state,
            ThreadState::Managed | ThreadState::CallbackManaged
        ) {
            return Err(GcError::InvalidTransition);
        }
        let id = self.next_object_id;
        self.next_object_id += 1;
        self.objects.push(Object {
            id,
            location: self.next_location,
            value,
        });
        self.next_location += 16;

        if self.allocation_threshold > 0
            && self.objects.len().is_multiple_of(self.allocation_threshold)
        {
            self.collect()?;
        }
        Ok(id)
    }

    pub fn create_handle(&mut self, object_id: u64) -> Result<StableHandle, GcError> {
        self.require_owner()?;
        let object = self
            .objects
            .iter()
            .find(|object| object.id == object_id)
            .ok_or(GcError::UnknownObject)?;
        let slot = self.handles.len();
        self.handles.push(HandleSlot {
            generation: 1,
            target_id: Some(object_id),
            target_location: object.location,
        });
        Ok(StableHandle {
            slot,
            generation: 1,
        })
    }

    pub fn read_handle(&self, handle: StableHandle) -> Result<u64, GcError> {
        self.require_owner()?;
        let slot = self
            .handles
            .get(handle.slot)
            .ok_or(GcError::InvalidHandle)?;
        if slot.generation != handle.generation {
            return Err(GcError::InvalidHandle);
        }
        let object_id = slot.target_id.ok_or(GcError::InvalidHandle)?;
        let object = self
            .objects
            .iter()
            .find(|object| object.id == object_id)
            .ok_or(GcError::InvalidHandle)?;
        if object.location != slot.target_location {
            return Err(GcError::InvalidHandle);
        }
        Ok(object.value)
    }

    pub fn release_handle(&mut self, handle: StableHandle) -> Result<(), GcError> {
        self.require_owner()?;
        let slot = self
            .handles
            .get_mut(handle.slot)
            .ok_or(GcError::InvalidHandle)?;
        if slot.generation != handle.generation || slot.target_id.is_none() {
            return Err(GcError::InvalidHandle);
        }
        slot.target_id = None;
        slot.target_location = 0;
        slot.generation = slot.generation.wrapping_add(1);
        Ok(())
    }

    pub fn enter_native_safe(&mut self) -> Result<(), GcError> {
        self.require_owner()?;
        if self.state != ThreadState::Managed {
            return Err(GcError::InvalidTransition);
        }
        self.roots_published = true;
        self.state = ThreadState::NativeSafe;
        Ok(())
    }

    pub fn begin_owner_sync_callback(&mut self) -> Result<(), GcError> {
        self.require_owner()?;
        if self.state != ThreadState::NativeSafe || !self.roots_published {
            return Err(GcError::InvalidTransition);
        }
        self.state = ThreadState::CallbackEntry;
        self.state = ThreadState::CallbackManaged;
        Ok(())
    }

    pub fn end_owner_sync_callback(&mut self) -> Result<(), GcError> {
        self.require_owner()?;
        if self.state != ThreadState::CallbackManaged {
            return Err(GcError::InvalidTransition);
        }
        self.state = ThreadState::NativeSafe;
        Ok(())
    }

    pub fn leave_native_safe(&mut self) -> Result<(), GcError> {
        self.require_owner()?;
        if self.state != ThreadState::NativeSafe {
            return Err(GcError::InvalidTransition);
        }
        self.roots_published = false;
        self.state = ThreadState::Managed;
        Ok(())
    }

    pub fn collect(&mut self) -> Result<(), GcError> {
        self.require_owner()?;
        if matches!(
            self.state,
            ThreadState::NativeSafe | ThreadState::CallbackEntry
        ) {
            return Err(GcError::CollectionDeferred);
        }

        self.collection_count += 1;
        let relocation_base = 1_000_000 + self.collection_count * 100_000;
        for (index, object) in self.objects.iter_mut().enumerate() {
            let index = u64::try_from(index).map_err(|_| GcError::InvalidTransition)?;
            object.location = relocation_base + index * 16;
        }
        for slot in &mut self.handles {
            if let Some(target_id) = slot.target_id {
                let object = self
                    .objects
                    .iter()
                    .find(|object| object.id == target_id)
                    .ok_or(GcError::InvalidHandle)?;
                slot.target_location = object.location;
            }
        }
        Ok(())
    }

    fn require_owner(&self) -> Result<(), GcError> {
        if thread::current().id() == self.owner {
            Ok(())
        } else {
            Err(GcError::WrongThread)
        }
    }
}
