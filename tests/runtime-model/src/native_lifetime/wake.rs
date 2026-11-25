use super::search::{Protocol, Violation, require};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum WakeFault {
    #[default]
    None,
    DetachOnPhysicalDone,
    DetachDuringNotify,
    FreeRetainedTarget,
}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Attachment, references, physical completion, and active accesses differ"
)]
pub struct Wake {
    pub(super) published: bool,
    pub(super) physical_done: bool,
    pub(super) helper: u8,
    pub(super) attached: bool,
    pub(super) context: bool,
    pub(super) producer_reference: bool,
    pub(super) notification: u8,
    pub(super) active_context: bool,
    pub(super) target: bool,
    channel: bool,
    pub fault: WakeFault,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Publish,
    AcquireAndCopy,
    HelperTail,
    HelperReturn,
    Join,
    BeginNotify,
    EndNotify,
    ReleaseProducerReference,
    Detach,
    FreeTarget,
}
impl Default for Wake {
    fn default() -> Self {
        Self::new()
    }
}
impl Wake {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            published: false,
            physical_done: false,
            helper: 0,
            attached: true,
            context: true,
            producer_reference: true,
            notification: 0,
            active_context: false,
            target: true,
            channel: false,
            fault: WakeFault::None,
        }
    }
    pub(super) fn channel() -> Self {
        Self {
            channel: true,
            ..Self::new()
        }
    }
}
impl Protocol for Wake {
    type Action = Action;
    fn name(&self) -> String {
        "wake-helper-and-delayed-producer".into()
    }
    fn actions(&self) -> Vec<Action> {
        use Action as A;
        let mut a = Vec::new();
        if !self.published {
            a.push(A::Publish);
        }
        if self.published && !self.physical_done {
            a.push(A::AcquireAndCopy);
        }
        if self.physical_done && self.helper == 0 {
            a.push(A::HelperTail);
        }
        if self.helper == 1 {
            a.push(A::HelperReturn);
        }
        if self.helper == 2 {
            a.push(A::Join);
        }
        if self.published && self.producer_reference && self.notification == 0 {
            a.push(A::BeginNotify);
        }
        if self.notification == 1 {
            a.push(A::EndNotify);
        }
        if self.notification == 2 && self.producer_reference {
            a.push(A::ReleaseProducerReference);
        }
        if self.attached
            && self.physical_done
            && (self.helper == 3
                || (self.channel && self.helper == 2)
                || self.fault == WakeFault::DetachOnPhysicalDone)
            && (!self.active_context || self.fault == WakeFault::DetachDuringNotify)
        {
            a.push(A::Detach);
        }
        if self.target
            && !self.attached
            && ((!self.producer_reference && self.notification != 1)
                || self.fault == WakeFault::FreeRetainedTarget)
        {
            a.push(A::FreeTarget);
        }
        a
    }
    fn apply(&mut self, a: Action) {
        use Action as A;
        match a {
            A::Publish => self.published = true,
            A::AcquireAndCopy => self.physical_done = true,
            A::HelperTail => self.helper = 1,
            A::HelperReturn => self.helper = 2,
            A::Join => self.helper = 3,
            A::BeginNotify => {
                self.notification = 1;
                self.active_context = self.attached && !self.channel;
            }
            A::EndNotify => {
                self.notification = 2;
                self.active_context = false;
            }
            A::ReleaseProducerReference => self.producer_reference = false,
            A::Detach => {
                self.attached = false;
                self.context = false;
            }
            A::FreeTarget => self.target = false,
        }
    }
    fn check(&self) -> Result<(), Violation> {
        require(
            !self.physical_done || self.published,
            "NL-I2",
            "runtime completion preceded SDK publication",
        )?;
        require(
            self.context || self.helper == 3 || (self.channel && self.helper == 2),
            "NL-I6",
            "helper wake tail can access a freed context",
        )?;
        require(
            self.context || !self.active_context,
            "NL-I7",
            "runtime detached during an active native notification",
        )?;
        require(
            self.target || (!self.attached && !self.producer_reference && self.notification != 1),
            "NL-I7",
            "wake target freed before its last reference/access",
        )?;
        Ok(())
    }
    fn terminal(&self) -> bool {
        !self.target
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.physical_done && self.helper < 2 {
            w.push("physical-done-before-wake-tail-return");
        }
        if !self.attached && self.notification == 1 && !self.active_context {
            w.push("late-notify-uses-target-without-runtime-context");
        }
        if !self.context && self.producer_reference {
            w.push("producer-reference-outlives-runtime");
        }
        w
    }
}
