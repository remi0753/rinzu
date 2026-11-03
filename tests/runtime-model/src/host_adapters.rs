//! Host Loop conformance fixtures for queue-post and frame-tick hosts.

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NestedMode {
    Root,
    Modal,
    FrameOverlay,
    Undeclared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterError {
    UnsupportedAtGeneration(NestedMode),
    UnsupportedAtCall(NestedMode),
    Closed,
    WrongGeneration,
}

pub trait HostAdapter {
    fn supports(&self, mode: NestedMode) -> bool;
    fn activate(&mut self, generation: u32);
    /// Request one owner turn for the active generation and mode.
    ///
    /// # Errors
    ///
    /// Rejects closed, stale-generation, and unsupported-mode calls.
    fn post_owner_turn(
        &mut self,
        generation: u32,
        mode: NestedMode,
        work_id: usize,
    ) -> Result<(), AdapterError>;
    fn drive_once(&mut self) -> Option<usize>;
    fn deactivate(&mut self);
}

/// Generator-side validation for a statically named nested mode.
///
/// # Errors
///
/// Returns `UnsupportedAtGeneration` instead of emitting an unsafe binding.
pub fn validate_declared_mode<A: HostAdapter>(
    adapter: &A,
    mode: NestedMode,
) -> Result<(), AdapterError> {
    if adapter.supports(mode) {
        Ok(())
    } else {
        Err(AdapterError::UnsupportedAtGeneration(mode))
    }
}

#[derive(Debug, Default)]
pub struct QueuePostAdapter {
    generation: Option<u32>,
    queue: VecDeque<usize>,
}

impl HostAdapter for QueuePostAdapter {
    fn supports(&self, mode: NestedMode) -> bool {
        matches!(mode, NestedMode::Root | NestedMode::Modal)
    }

    fn activate(&mut self, generation: u32) {
        self.generation = Some(generation);
    }

    fn post_owner_turn(
        &mut self,
        generation: u32,
        mode: NestedMode,
        work_id: usize,
    ) -> Result<(), AdapterError> {
        let Some(active) = self.generation else {
            return Err(AdapterError::Closed);
        };
        if active != generation {
            return Err(AdapterError::WrongGeneration);
        }
        if !self.supports(mode) {
            return Err(AdapterError::UnsupportedAtCall(mode));
        }
        self.queue.push_back(work_id);
        Ok(())
    }

    fn drive_once(&mut self) -> Option<usize> {
        self.queue.pop_front()
    }

    fn deactivate(&mut self) {
        self.generation = None;
    }
}

#[derive(Debug, Default)]
pub struct FrameTickAdapter {
    generation: Option<u32>,
    pending: VecDeque<usize>,
    frame: u64,
}

impl HostAdapter for FrameTickAdapter {
    fn supports(&self, mode: NestedMode) -> bool {
        matches!(mode, NestedMode::Root | NestedMode::FrameOverlay)
    }

    fn activate(&mut self, generation: u32) {
        self.generation = Some(generation);
    }

    fn post_owner_turn(
        &mut self,
        generation: u32,
        mode: NestedMode,
        work_id: usize,
    ) -> Result<(), AdapterError> {
        let Some(active) = self.generation else {
            return Err(AdapterError::Closed);
        };
        if active != generation {
            return Err(AdapterError::WrongGeneration);
        }
        if !self.supports(mode) {
            return Err(AdapterError::UnsupportedAtCall(mode));
        }
        self.pending.push_back(work_id);
        Ok(())
    }

    fn drive_once(&mut self) -> Option<usize> {
        self.frame += 1;
        // One bounded managed turn per host frame.
        self.pending.pop_front()
    }

    fn deactivate(&mut self) {
        self.generation = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConformanceReport {
    pub posted: usize,
    pub delivered: usize,
    pub duplicate_deliveries: usize,
}

/// Shared lifecycle/delivery suite used by every host driving strategy.
///
/// # Panics
///
/// Panics only if an adapter violates the declared root-mode contract.
#[must_use]
pub fn run_common_conformance<A: HostAdapter>(adapter: &mut A) -> ConformanceReport {
    const COUNT: usize = 32;
    adapter.activate(7);
    for id in 0..COUNT {
        adapter.post_owner_turn(7, NestedMode::Root, id).unwrap();
    }
    let mut seen = [false; COUNT];
    let mut delivered = 0;
    let mut duplicates = 0;
    while let Some(id) = adapter.drive_once() {
        if seen[id] {
            duplicates += 1;
        } else {
            seen[id] = true;
            delivered += 1;
        }
    }
    assert_eq!(
        adapter.post_owner_turn(8, NestedMode::Root, COUNT),
        Err(AdapterError::WrongGeneration)
    );
    adapter.deactivate();
    assert_eq!(
        adapter.post_owner_turn(7, NestedMode::Root, COUNT),
        Err(AdapterError::Closed)
    );
    ConformanceReport {
        posted: COUNT,
        delivered,
        duplicate_deliveries: duplicates,
    }
}
