//! Versioned dispatch and development-runner process restart prototype.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Eq, PartialEq)]
struct CodeVersion {
    id: u64,
    result: i64,
}

#[derive(Clone, Debug)]
pub struct DispatchCell {
    current: Arc<RwLock<Arc<CodeVersion>>>,
}

impl DispatchCell {
    #[must_use]
    pub fn new(result: i64) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(CodeVersion { id: 1, result }))),
        }
    }

    #[must_use]
    pub fn enter_call(&self) -> Frame {
        Frame {
            code: Arc::clone(
                &self
                    .current
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            ),
            suspended: false,
        }
    }

    /// Install a body-compatible code version.
    ///
    /// # Errors
    ///
    /// Returns `ProcessRestartRequired` for signature, layout, ABI, capture,
    /// or transfer-descriptor incompatibility.
    pub fn reload_body(&self, compatible: bool, result: i64) -> Result<u64, ReloadError> {
        if !compatible {
            return Err(ReloadError::ProcessRestartRequired);
        }
        let mut current = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = current.id + 1;
        *current = Arc::new(CodeVersion { id, result });
        Ok(id)
    }

    #[must_use]
    pub fn current_version(&self) -> u64 {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadError {
    ProcessRestartRequired,
}

#[derive(Clone, Debug)]
pub struct Frame {
    code: Arc<CodeVersion>,
    suspended: bool,
}

impl Frame {
    pub fn suspend(&mut self) {
        self.suspended = true;
    }

    pub fn resume(&mut self) {
        self.suspended = false;
    }

    #[must_use]
    pub fn version(&self) -> u64 {
        self.code.id
    }

    #[must_use]
    pub fn execute(&self) -> i64 {
        self.code.result
    }

    #[must_use]
    pub const fn is_suspended(&self) -> bool {
        self.suspended
    }
}

#[derive(Clone, Debug)]
pub struct CallbackTrampoline {
    dispatch: DispatchCell,
}

impl CallbackTrampoline {
    #[must_use]
    pub const fn new(dispatch: DispatchCell) -> Self {
        Self { dispatch }
    }

    #[must_use]
    pub fn invoke(&self) -> (u64, i64) {
        let frame = self.dispatch.enter_call();
        (frame.version(), frame.execute())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainStatus {
    AtSafePoint,
    ManagedRunning,
    NativeBlocking,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartDecision {
    InProcessReload,
    ProcessRestartNativeBlocker,
}

#[must_use]
pub fn decide_restart(statuses: &[DomainStatus]) -> RestartDecision {
    if statuses.contains(&DomainStatus::NativeBlocking) {
        RestartDecision::ProcessRestartNativeBlocker
    } else {
        RestartDecision::InProcessReload
    }
}

/// Terminate an unresponsive development child and run a replacement.
///
/// # Errors
///
/// Propagates OS process-control and replacement-spawn errors.
pub fn perform_process_restart(
    old_process: &mut Child,
    replacement: &mut Command,
) -> io::Result<ExitStatus> {
    old_process.kill()?;
    let _ = old_process.wait()?;
    replacement.spawn()?.wait()
}
