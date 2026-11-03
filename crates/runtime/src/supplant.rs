//! One Main supplant reservation, retained until callback/resource settlement.
//! Entry and cancellation choose their order under one gate. No gate lock
//! spans native code, callback execution or origin cleanup.
use std::sync::{Mutex, MutexGuard};
use std::thread::ThreadId;

fn fatal() -> ! {
    // SAFETY: protocol violations cannot unwind, flush output or run cleanup.
    unsafe { libc::_exit(86) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Generation(u64);
impl Generation {
    pub const fn bits(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Idle,
    Preparing,
    Queued,
    Entering,
    Running,
    CancelRequested,
    Settling,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Winner {
    Returned,
    Failed,
    Cancelled,
    Shutdown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cancellation {
    Task,
    Shutdown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupplantError {
    Busy,
    WrongThread,
    StaleGeneration,
    InvalidPhase,
}
impl std::fmt::Display for SupplantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "supplant protocol violation: {self:?}")
    }
}
impl std::error::Error for SupplantError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub generation: Option<Generation>,
    pub phase: Phase,
    pub winner: Option<Winner>,
    pub entered: bool,
}
#[derive(Debug)]
struct State {
    next_generation: u64,
    current: Snapshot,
}
#[derive(Debug)]
pub struct SupplantCoordinator {
    owner: ThreadId,
    gate: Mutex<State>,
}
impl Default for SupplantCoordinator {
    fn default() -> Self {
        Self::new()
    }
}
impl SupplantCoordinator {
    pub fn new() -> Self {
        Self {
            owner: std::thread::current().id(),
            gate: Mutex::new(State {
                next_generation: 0,
                current: Snapshot {
                    generation: None,
                    phase: Phase::Idle,
                    winner: None,
                    entered: false,
                },
            }),
        }
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        self.gate.lock().unwrap_or_else(|_| fatal())
    }
    fn owner(&self) -> Result<(), SupplantError> {
        if self.owner == std::thread::current().id() {
            Ok(())
        } else {
            Err(SupplantError::WrongThread)
        }
    }
    fn active(state: &mut State, generation: Generation) -> Result<&mut Snapshot, SupplantError> {
        if state.current.generation == Some(generation) {
            Ok(&mut state.current)
        } else {
            Err(SupplantError::StaleGeneration)
        }
    }
    pub fn reserve(&self) -> Result<Generation, SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        if state.current.phase != Phase::Idle {
            return Err(SupplantError::Busy);
        }
        state.next_generation = state
            .next_generation
            .checked_add(1)
            .unwrap_or_else(|| fatal());
        let generation = Generation(state.next_generation);
        state.current = Snapshot {
            generation: Some(generation),
            phase: Phase::Preparing,
            winner: None,
            entered: false,
        };
        Ok(generation)
    }
    /// False means cancellation already won before physical entry.
    pub fn queue(&self, generation: Generation) -> Result<bool, SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        let active = Self::active(&mut state, generation)?;
        if active.phase == Phase::Settling && !active.entered {
            return Ok(false);
        }
        if active.phase != Phase::Preparing {
            return Err(SupplantError::InvalidPhase);
        }
        active.phase = Phase::Queued;
        Ok(true)
    }
    /// Owner start turn, after the whole synchronous managed chain returned.
    /// Once this edge wins, cancellation waits for the real native return.
    pub fn begin_entry(&self, generation: Generation) -> Result<bool, SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        let active = Self::active(&mut state, generation)?;
        if active.phase == Phase::Settling && !active.entered {
            return Ok(false);
        }
        if active.phase != Phase::Queued {
            return Err(SupplantError::InvalidPhase);
        }
        active.phase = Phase::Entering;
        Ok(true)
    }
    pub fn entered(&self, generation: Generation) -> Result<(), SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        let active = Self::active(&mut state, generation)?;
        if active.entered || !matches!(active.phase, Phase::Entering | Phase::CancelRequested) {
            return Err(SupplantError::InvalidPhase);
        }
        active.entered = true;
        if active.phase == Phase::Entering {
            active.phase = Phase::Running;
        }
        Ok(())
    }
    /// Safe from a cancellation notifier thread; stale requests cannot touch
    /// a later call. Default Ctrl-C is handled by immediate process exit.
    pub fn cancel(&self, generation: Generation, cancellation: Cancellation) -> bool {
        let mut state = self.lock();
        let Ok(active) = Self::active(&mut state, generation) else {
            return false;
        };
        if active.winner.is_some() {
            return false;
        }
        active.winner = Some(match cancellation {
            Cancellation::Task => Winner::Cancelled,
            Cancellation::Shutdown => Winner::Shutdown,
        });
        active.phase = match active.phase {
            Phase::Preparing | Phase::Queued => Phase::Settling,
            Phase::Entering | Phase::Running => Phase::CancelRequested,
            _ => fatal(),
        };
        true
    }
    /// Retain the failed preparation's resources until its origin settles them.
    pub fn fail_preparation(&self, generation: Generation) -> Result<bool, SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        let active = Self::active(&mut state, generation)?;
        if active.winner.is_some() {
            return Ok(false);
        }
        if !matches!(active.phase, Phase::Preparing | Phase::Queued) {
            return Err(SupplantError::InvalidPhase);
        }
        active.winner = Some(Winner::Failed);
        active.phase = Phase::Settling;
        Ok(true)
    }
    pub fn returned(&self, generation: Generation) -> Result<(), SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        let active = Self::active(&mut state, generation)?;
        if !active.entered || !matches!(active.phase, Phase::Running | Phase::CancelRequested) {
            return Err(SupplantError::InvalidPhase);
        }
        active.winner.get_or_insert(Winner::Returned);
        active.phase = Phase::Settling;
        Ok(())
    }
    /// The Owner calls this only after every admitted callback and call
    /// resource has settled. Result roots remain with the continuation.
    pub fn finish_settlement(&self, generation: Generation) -> Result<Winner, SupplantError> {
        self.owner()?;
        let mut state = self.lock();
        let active = Self::active(&mut state, generation)?;
        if active.phase != Phase::Settling {
            return Err(SupplantError::InvalidPhase);
        }
        let winner = active.winner.ok_or(SupplantError::InvalidPhase)?;
        *active = Snapshot {
            generation: None,
            phase: Phase::Idle,
            winner: None,
            entered: false,
        };
        Ok(winner)
    }
    pub fn snapshot(&self) -> Snapshot {
        self.lock().current
    }
}
