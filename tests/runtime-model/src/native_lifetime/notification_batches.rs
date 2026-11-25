use super::search::{Violation, require};
use super::{Binding, Protocol};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Wake {
    Capturing,
    Merged,
    Attached,
    Detached,
    Notified,
    Returned,
}

impl Wake {
    const fn references(self) -> usize {
        match self {
            Self::Capturing => 2,
            Self::Returned => 0,
            _ => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
enum Runtime {
    #[default]
    Attached,
    Detached,
    ReferenceReturned,
    Destroyed,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Batch {
    published: u8,
    settled: u8,
    wake: Wake,
}

impl Default for Batch {
    fn default() -> Self {
        Self {
            published: 0,
            settled: 0,
            wake: Wake::Capturing,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationAction {
    Capture(usize, usize),
    Merge(usize),
    Settle(usize, usize),
    Begin(usize),
    End(usize),
    Return(usize),
    Detach,
    ReturnRuntime,
    Destroy,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NotificationBatches<const N: usize> {
    batches: [Batch; N],
    binding: Binding,
    runtime: Runtime,
    references: usize,
}

impl<const N: usize> NotificationBatches<N> {
    /// There are two distinct, registered carriers in each batch, with one SDK
    /// slot reference each. All batches share the same live target.
    #[must_use]
    pub fn new(binding: Binding) -> Self {
        Self {
            batches: [Batch::default(); N],
            binding,
            runtime: Runtime::Attached,
            references: 1 + N * 2,
        }
    }

    #[must_use]
    pub fn references(&self) -> usize {
        self.references
    }

    #[must_use]
    pub fn environment_attached(&self) -> bool {
        self.runtime == Runtime::Attached
    }
}

impl<const N: usize> Protocol for NotificationBatches<N> {
    type Action = NotificationAction;

    fn name(&self) -> String {
        format!(
            "{:?}-{N}-pairs-sharing-one-notification-target",
            self.binding
        )
    }

    fn actions(&self) -> Vec<Self::Action> {
        use NotificationAction as A;
        let mut actions = Vec::new();
        let attached_callback = self
            .batches
            .iter()
            .any(|batch| batch.wake == Wake::Attached);
        let active_callback = self
            .batches
            .iter()
            .any(|batch| matches!(batch.wake, Wake::Attached | Wake::Detached));
        for (index, batch) in self.batches.iter().enumerate() {
            for member in 0..2 {
                let bit = 1 << member;
                if batch.published & bit == 0 {
                    actions.push(A::Capture(index, member));
                }
                if batch.published & bit != 0 && batch.settled & bit == 0 {
                    actions.push(A::Settle(index, member));
                }
            }
            match batch.wake {
                Wake::Capturing if batch.published == 3 => actions.push(A::Merge(index)),
                Wake::Merged if !active_callback || self.binding == Binding::RustChannel => {
                    actions.push(A::Begin(index));
                }
                Wake::Attached | Wake::Detached => actions.push(A::End(index)),
                Wake::Notified => actions.push(A::Return(index)),
                _ => {}
            }
        }
        let has_context_reader = self.binding != Binding::RustChannel && attached_callback;
        if self.runtime == Runtime::Attached
            && !has_context_reader
            && self.batches.iter().all(|batch| batch.settled == 3)
        {
            actions.push(A::Detach);
        }
        if self.runtime == Runtime::Detached {
            actions.push(A::ReturnRuntime);
        }
        if self.runtime == Runtime::ReferenceReturned && self.references == 0 {
            actions.push(A::Destroy);
        }
        actions
    }

    fn apply(&mut self, action: Self::Action) {
        use NotificationAction as A;
        match action {
            A::Capture(b, member) => self.batches[b].published |= 1 << member,
            A::Settle(b, member) => self.batches[b].settled |= 1 << member,
            A::Merge(b) => {
                self.batches[b].wake = Wake::Merged;
                self.references -= 1;
            }
            A::Begin(b) => {
                self.batches[b].wake = if self.runtime == Runtime::Attached {
                    Wake::Attached
                } else {
                    Wake::Detached
                };
            }
            A::End(b) => self.batches[b].wake = Wake::Notified,
            A::Return(b) => {
                self.batches[b].wake = Wake::Returned;
                self.references -= 1;
            }
            A::Detach => self.runtime = Runtime::Detached,
            A::ReturnRuntime => {
                self.runtime = Runtime::ReferenceReturned;
                self.references -= 1;
            }
            A::Destroy => self.runtime = Runtime::Destroyed,
        }
    }

    fn check(&self) -> Result<(), Violation> {
        let sdk: usize = self
            .batches
            .iter()
            .map(|batch| batch.wake.references())
            .sum();
        let runtime = usize::from(matches!(
            self.runtime,
            Runtime::Attached | Runtime::Detached
        ));
        require(
            self.references == runtime + sdk,
            "NL-I7",
            "the target count differs from the runtime and retained SDK leases",
        )?;
        require(
            self.batches
                .iter()
                .all(|batch| batch.settled & !batch.published == 0),
            "NL-I2",
            "settled a receipt before its SDK exchange",
        )?;
        require(
            self.runtime == Runtime::Attached
                || self.batches.iter().all(|batch| {
                    batch.settled == 3
                        && (self.binding == Binding::RustChannel || batch.wake != Wake::Attached)
                }),
            "NL-I5",
            "released an environment with a physical user or attached notification",
        )?;
        require(
            self.runtime != Runtime::Destroyed || self.references == 0,
            "NL-I7",
            "destroyed a retained target",
        )
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        use NotificationAction as A;
        match action {
            A::Merge(b) | A::Return(b) => require(
                self.batches
                    .iter()
                    .zip(after.batches)
                    .enumerate()
                    .all(|(i, (before, after))| i == b || *before == after)
                    && self.runtime == after.runtime,
                "NL-I7",
                "returning an SDK lease touched a different batch or the runtime",
            ),
            A::Detach => require(
                self.runtime == Runtime::Attached
                    && after.runtime == Runtime::Detached
                    && self.references == after.references
                    && self.batches == after.batches,
                "NL-I7",
                "detaching the environment returned or changed a reference",
            ),
            A::ReturnRuntime => require(
                self.runtime == Runtime::Detached
                    && after.runtime == Runtime::ReferenceReturned
                    && self.references.checked_sub(1) == Some(after.references)
                    && self.batches == after.batches,
                "NL-I7",
                "the runtime returned a reference before detaching or changed an SDK lease",
            ),
            _ => Ok(()),
        }
    }

    fn terminal(&self) -> bool {
        self.runtime == Runtime::Destroyed
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut witnesses = Vec::new();
        if self
            .batches
            .iter()
            .filter(|b| b.wake == Wake::Attached)
            .count()
            >= 2
        {
            witnesses.push("concurrent-attached-notifications");
        }
        if self.batches.iter().any(|b| b.wake == Wake::Detached) {
            witnesses.push("notify-after-detach");
            if self.runtime == Runtime::Detached {
                witnesses.push("notify-before-runtime-reference-return");
            }
        }
        if self.runtime == Runtime::Detached && self.references == 1 {
            witnesses.push("runtime-can-return-the-last-reference");
        }
        if self.runtime != Runtime::Attached
            && self.batches.iter().any(|b| b.wake == Wake::Attached)
        {
            witnesses.push("detached-during-channel-notification");
        }
        if self.batches.iter().all(|b| b.wake == Wake::Merged) {
            witnesses.push("all-batches-retain-independent-leaders");
        }
        witnesses
    }
}
