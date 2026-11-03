//! Hard resource accounting for runtime-owned work.

use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ResourceKind {
    LiveTask,
    PendingOperation,
    Registration,
    QueueSlot,
    ControlSlot,
}

impl ResourceKind {
    const ALL: [Self; 5] = [
        Self::LiveTask,
        Self::PendingOperation,
        Self::Registration,
        Self::QueueSlot,
        Self::ControlSlot,
    ];

    const fn index(self) -> usize {
        match self {
            Self::LiveTask => 0,
            Self::PendingOperation => 1,
            Self::Registration => 2,
            Self::QueueSlot => 3,
            Self::ControlSlot => 4,
        }
    }
    const fn accounting(self) -> crate::resource_accounting::Kind {
        use crate::resource_accounting::Kind;
        match self {
            Self::LiveTask => Kind::Task,
            Self::PendingOperation => Kind::PendingOperation,
            Self::Registration => Kind::Registration,
            Self::QueueSlot => Kind::Queue,
            Self::ControlSlot => Kind::RuntimeControl,
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::LiveTask => "live task",
            Self::PendingOperation => "pending operation",
            Self::Registration => "registration",
            Self::QueueSlot => "queue slot",
            Self::ControlSlot => "control slot",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceLimits {
    pub live_tasks: usize,
    pub pending_operations: usize,
    pub registrations: usize,
    pub queue_slots: usize,
    pub control_slots: usize,
}

impl ResourceLimits {
    #[must_use]
    pub const fn limit(self, kind: ResourceKind) -> usize {
        match kind {
            ResourceKind::LiveTask => self.live_tasks,
            ResourceKind::PendingOperation => self.pending_operations,
            ResourceKind::Registration => self.registrations,
            ResourceKind::QueueSlot => self.queue_slots,
            ResourceKind::ControlSlot => self.control_slots,
        }
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            live_tasks: 4_096,
            pending_operations: 8_192,
            registrations: 1_024,
            queue_slots: 12_288,
            control_slots: 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceUse {
    pub live_tasks: usize,
    pub pending_operations: usize,
    pub registrations: usize,
    pub queue_slots: usize,
    pub control_slots: usize,
}

impl ResourceUse {
    #[must_use]
    pub const fn count(self, kind: ResourceKind) -> usize {
        match kind {
            ResourceKind::LiveTask => self.live_tasks,
            ResourceKind::PendingOperation => self.pending_operations,
            ResourceKind::Registration => self.registrations,
            ResourceKind::QueueSlot => self.queue_slots,
            ResourceKind::ControlSlot => self.control_slots,
        }
    }

    #[must_use]
    pub const fn is_baseline(self) -> bool {
        self.live_tasks == 0
            && self.pending_operations == 0
            && self.registrations == 0
            && self.queue_slots == 0
            && self.control_slots == 0
    }

    fn from_counts(counts: [usize; 5]) -> Self {
        Self {
            live_tasks: counts[ResourceKind::LiveTask.index()],
            pending_operations: counts[ResourceKind::PendingOperation.index()],
            registrations: counts[ResourceKind::Registration.index()],
            queue_slots: counts[ResourceKind::QueueSlot.index()],
            control_slots: counts[ResourceKind::ControlSlot.index()],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceSnapshot {
    pub limits: ResourceLimits,
    pub used: ResourceUse,
    pub peak: ResourceUse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceError {
    Limit {
        resource: ResourceKind,
        requested: usize,
        used: usize,
        limit: usize,
    },
    ReleaseUnderflow {
        resource: ResourceKind,
        released: usize,
        used: usize,
    },
}

impl fmt::Display for ResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit {
                resource,
                requested,
                used,
                limit,
            } => write!(
                formatter,
                "{resource} resource limit exceeded: requested {requested}, used {used}, limit {limit}"
            ),
            Self::ReleaseUnderflow {
                resource,
                released,
                used,
            } => write!(
                formatter,
                "{resource} resource ledger underflow: released {released}, used {used}"
            ),
        }
    }
}

impl std::error::Error for ResourceError {}

#[derive(Debug)]
pub struct ResourceLedger {
    limits: ResourceLimits,
    used: [usize; 5],
    peak: [usize; 5],
    accounting: bool,
}

impl ResourceLedger {
    #[must_use]
    pub const fn new(limits: ResourceLimits) -> Self {
        Self {
            limits,
            used: [0; 5],
            peak: [0; 5],
            accounting: false,
        }
    }

    /// Reserves all requested credits or none of them.
    pub fn reserve(&mut self, credits: &[(ResourceKind, usize)]) -> Result<(), ResourceError> {
        let requested = Self::aggregate(credits);
        for kind in ResourceKind::ALL {
            let index = kind.index();
            let Some(total) = self.used[index].checked_add(requested[index]) else {
                return Err(ResourceError::Limit {
                    resource: kind,
                    requested: requested[index],
                    used: self.used[index],
                    limit: self.limits.limit(kind),
                });
            };
            if total > self.limits.limit(kind) {
                return Err(ResourceError::Limit {
                    resource: kind,
                    requested: requested[index],
                    used: self.used[index],
                    limit: self.limits.limit(kind),
                });
            }
        }
        if self.used == [0; 5] {
            self.accounting = crate::resource_accounting::enabled();
        }
        for kind in ResourceKind::ALL {
            let index = kind.index();
            self.used[index] += requested[index];
            self.peak[index] = self.peak[index].max(self.used[index]);
            if self.accounting {
                crate::resource_accounting::acquire_recorded(
                    kind.accounting(),
                    requested[index] as u64,
                );
            }
        }
        Ok(())
    }

    /// Releases all requested credits or none of them.
    pub fn release(&mut self, credits: &[(ResourceKind, usize)]) -> Result<(), ResourceError> {
        let released = Self::aggregate(credits);
        for kind in ResourceKind::ALL {
            let index = kind.index();
            if released[index] > self.used[index] {
                return Err(ResourceError::ReleaseUnderflow {
                    resource: kind,
                    released: released[index],
                    used: self.used[index],
                });
            }
        }
        for kind in ResourceKind::ALL {
            let index = kind.index();
            self.used[index] -= released[index];
            if self.accounting {
                crate::resource_accounting::release(kind.accounting(), released[index] as u64);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            limits: self.limits,
            used: ResourceUse::from_counts(self.used),
            peak: ResourceUse::from_counts(self.peak),
        }
    }

    fn aggregate(credits: &[(ResourceKind, usize)]) -> [usize; 5] {
        let mut counts = [0_usize; 5];
        for (kind, count) in credits {
            counts[kind.index()] = counts[kind.index()]
                .checked_add(*count)
                .expect("a resource request count must fit in usize");
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ResourceLimits {
        ResourceLimits {
            live_tasks: 1,
            pending_operations: 1,
            registrations: 1,
            queue_slots: 2,
            control_slots: 1,
        }
    }

    #[test]
    fn every_resource_has_a_hard_limit_and_peak() {
        for kind in ResourceKind::ALL {
            let mut ledger = ResourceLedger::new(limits());
            let limit = limits().limit(kind);
            ledger.reserve(&[(kind, limit)]).unwrap();
            assert!(matches!(
                ledger.reserve(&[(kind, 1)]),
                Err(ResourceError::Limit { resource, .. }) if resource == kind
            ));
            assert_eq!(ledger.snapshot().peak.count(kind), limit);
            ledger.release(&[(kind, limit)]).unwrap();
            assert!(ledger.snapshot().used.is_baseline());
        }
    }

    #[test]
    fn multi_resource_admission_is_all_or_nothing() {
        let mut ledger = ResourceLedger::new(limits());
        ledger
            .reserve(&[(ResourceKind::LiveTask, 1), (ResourceKind::QueueSlot, 1)])
            .unwrap();
        let before = ledger.snapshot().used;
        assert!(matches!(
            ledger.reserve(&[
                (ResourceKind::PendingOperation, 1),
                (ResourceKind::QueueSlot, 2),
            ]),
            Err(ResourceError::Limit {
                resource: ResourceKind::QueueSlot,
                ..
            })
        ));
        assert_eq!(ledger.snapshot().used, before);
        assert!(matches!(
            ledger.release(&[(ResourceKind::ControlSlot, 1)]),
            Err(ResourceError::ReleaseUnderflow {
                resource: ResourceKind::ControlSlot,
                ..
            })
        ));
    }
}
