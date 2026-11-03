//! Versioned indirection used by managed calls and native callback trampolines.

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchTarget {
    pub address: usize,
    pub version: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchError {
    NullAddress,
    Unpublished,
    VersionExhausted,
}

impl fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NullAddress => formatter.write_str("dispatch target address is null"),
            Self::Unpublished => formatter.write_str("dispatch cell has no published target"),
            Self::VersionExhausted => formatter.write_str("dispatch version is exhausted"),
        }
    }
}

impl std::error::Error for DispatchError {}

#[derive(Debug)]
pub struct VersionedDispatchCell {
    address: AtomicUsize,
    version: AtomicU64,
}

impl VersionedDispatchCell {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            address: AtomicUsize::new(0),
            version: AtomicU64::new(0),
        }
    }

    pub fn publish(&self, address: usize) -> Result<DispatchTarget, DispatchError> {
        if address == 0 {
            return Err(DispatchError::NullAddress);
        }
        let sequence = loop {
            let current = self.version.load(Ordering::Acquire);
            if current & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let Some(publishing) = current.checked_add(1) else {
                return Err(DispatchError::VersionExhausted);
            };
            if publishing.checked_add(1).is_none() {
                return Err(DispatchError::VersionExhausted);
            }
            if self
                .version
                .compare_exchange(current, publishing, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break current;
            }
        };
        self.address.store(address, Ordering::Release);
        let published = sequence + 2;
        self.version.store(published, Ordering::Release);
        let version = published / 2;
        Ok(DispatchTarget { address, version })
    }

    pub fn load(&self) -> Result<DispatchTarget, DispatchError> {
        loop {
            let version_before = self.version.load(Ordering::Acquire);
            if version_before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let address = self.address.load(Ordering::Acquire);
            let version_after = self.version.load(Ordering::Acquire);
            if version_before != version_after {
                continue;
            }
            if version_after == 0 || address == 0 {
                return Err(DispatchError::Unpublished);
            }
            return Ok(DispatchTarget {
                address,
                version: version_after / 2,
            });
        }
    }
}

impl Default for VersionedDispatchCell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_retargets_future_loads_without_changing_cell_identity() {
        let cell = VersionedDispatchCell::new();
        assert_eq!(cell.load(), Err(DispatchError::Unpublished));
        assert_eq!(
            cell.publish(0x1000).unwrap(),
            DispatchTarget {
                address: 0x1000,
                version: 1
            }
        );
        assert_eq!(
            cell.publish(0x2000).unwrap(),
            DispatchTarget {
                address: 0x2000,
                version: 2
            }
        );
        assert_eq!(
            cell.load().unwrap(),
            DispatchTarget {
                address: 0x2000,
                version: 2
            }
        );
    }
}
