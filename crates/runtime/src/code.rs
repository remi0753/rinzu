//! Published machine-code lifetime tracking.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const PUBLISHED: u8 = 0;
const RETIRING: u8 = 1;
const RETIRED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CodeVersionId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeVersionState {
    Published,
    Retiring,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeVersionError {
    NullAddress,
    Retired,
}

impl fmt::Display for CodeVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullAddress => formatter.write_str("code version address is null"),
            Self::Retired => formatter.write_str("code version is no longer enterable"),
        }
    }
}

impl std::error::Error for CodeVersionError {}

#[derive(Debug)]
pub struct CodeVersion {
    id: CodeVersionId,
    address: usize,
    size: usize,
    state: AtomicU8,
    live_frames: AtomicUsize,
    peak_live_frames: AtomicUsize,
}

impl CodeVersion {
    pub fn published(
        id: CodeVersionId,
        address: usize,
        size: usize,
    ) -> Result<Arc<Self>, CodeVersionError> {
        if address == 0 {
            return Err(CodeVersionError::NullAddress);
        }
        Ok(Arc::new(Self {
            id,
            address,
            size,
            state: AtomicU8::new(PUBLISHED),
            live_frames: AtomicUsize::new(0),
            peak_live_frames: AtomicUsize::new(0),
        }))
    }

    #[must_use]
    pub fn id(&self) -> CodeVersionId {
        self.id
    }

    #[must_use]
    pub fn address(&self) -> usize {
        self.address
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.size
    }

    #[must_use]
    pub fn state(&self) -> CodeVersionState {
        match self.state.load(Ordering::Acquire) {
            PUBLISHED => CodeVersionState::Published,
            RETIRING => CodeVersionState::Retiring,
            RETIRED => CodeVersionState::Retired,
            _ => unreachable!("code state is only written by this type"),
        }
    }

    #[must_use]
    pub fn live_frames(&self) -> usize {
        self.live_frames.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn peak_live_frames(&self) -> usize {
        self.peak_live_frames.load(Ordering::Acquire)
    }

    pub fn enter(self: &Arc<Self>) -> Result<CodeFrame, CodeVersionError> {
        loop {
            if self.state.load(Ordering::Acquire) != PUBLISHED {
                return Err(CodeVersionError::Retired);
            }
            let previous = self.live_frames.fetch_add(1, Ordering::AcqRel);
            if self.state.load(Ordering::Acquire) == PUBLISHED {
                let live = previous + 1;
                self.peak_live_frames.fetch_max(live, Ordering::AcqRel);
                return Ok(CodeFrame {
                    version: Arc::clone(self),
                });
            }
            self.release_frame();
        }
    }

    pub fn request_retire(&self) {
        if self
            .state
            .compare_exchange(PUBLISHED, RETIRING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && self.live_frames.load(Ordering::Acquire) == 0
        {
            let _ =
                self.state
                    .compare_exchange(RETIRING, RETIRED, Ordering::AcqRel, Ordering::Acquire);
        }
    }

    fn release_frame(&self) {
        let previous = self.live_frames.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "code frame count cannot underflow");
        if previous == 1 && self.state.load(Ordering::Acquire) == RETIRING {
            let _ =
                self.state
                    .compare_exchange(RETIRING, RETIRED, Ordering::AcqRel, Ordering::Acquire);
        }
    }
}

#[derive(Debug)]
pub struct CodeFrame {
    version: Arc<CodeVersion>,
}

impl CodeFrame {
    #[must_use]
    pub fn version(&self) -> &Arc<CodeVersion> {
        &self.version
    }
}

impl Drop for CodeFrame {
    fn drop(&mut self) {
        self.version.release_frame();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::VersionedDispatchCell;

    #[test]
    fn retiring_code_waits_for_old_live_frames() {
        let cell = VersionedDispatchCell::new();
        let old = CodeVersion::published(CodeVersionId(1), 0x1000, 64).unwrap();
        cell.publish(old.address()).unwrap();
        let frame = old.enter().unwrap();

        let new = CodeVersion::published(CodeVersionId(2), 0x2000, 80).unwrap();
        cell.publish(new.address()).unwrap();
        old.request_retire();
        assert_eq!(old.state(), CodeVersionState::Retiring);
        assert_eq!(old.live_frames(), 1);
        assert_eq!(cell.load().unwrap().address, new.address());

        drop(frame);
        assert_eq!(old.state(), CodeVersionState::Retired);
        assert_eq!(old.live_frames(), 0);
        assert!(new.enter().is_ok());
    }
}
