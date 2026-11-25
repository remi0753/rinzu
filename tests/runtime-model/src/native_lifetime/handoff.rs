use super::admission::{Action as A, Owner};
use super::call::{Action as K, Callback, Stage};
use super::search::{Violation, require};
use super::{
    Admission, Binding, Call, ComposedAction as C, Decision, Protocol, ReceiptStage, Registration,
    RegistrationAction as R,
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum HandoffFault {
    #[default]
    None,
    ReturnPublishesSdk,
    LoseDeferredDrain,
    RetryLosesDecision,
    ReleasePinWithCall,
    DisposeOnCancel,
    AdmitBeforeRead,
}

/// One physically entered call hands one independently pinned v3 carrier to one
/// drain. Other Call root/capability axes are checked by the component model.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Call, carrier and Owner have independent lifetimes"
)]
pub struct Handoff {
    pub(super) call: Call,
    pub(super) admission: Admission,
    pub(super) runtime: Option<Registration>,
    pub(super) binding: Binding,
    pub(super) full: bool,
    pub(super) sdk: u8,
    pub(super) started: bool,
    pub(super) handed: bool,
    pub(super) decision: Decision,
    pub(super) disposed: bool,
    pub(super) pin: bool,
    pub(super) unused: u8,
    pub fault: HandoffFault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffAction {
    Call(K),
    StartSdk,
    HandOff,
    TryAdmit,
    Credit,
    Retry,
    Control(Decision),
    SdkWrite,
    SdkPrefix,
    SdkPublish,
    DirectCopy,
    ReleaseDrain,
    ReleasePin,
    Runtime(R),
    Unused(u8),
    Dispose,
}

impl Handoff {
    /// The saturated profile is meaningful only for Rust's ordinary mailbox.
    /// # Panics
    /// Panics for a `CJoined` binding or saturated C profile.
    #[must_use]
    pub fn new(binding: Binding, full: bool) -> Self {
        assert!(binding != Binding::CJoined && (!full || binding == Binding::RustChannel));
        Self {
            call: Call::new(1, false),
            admission: Admission::new(full),
            runtime: None,
            binding,
            full,
            sdk: 0,
            started: false,
            handed: false,
            decision: Decision::Pending,
            disposed: false,
            pin: false,
            unused: 0,
            fault: HandoffFault::None,
        }
    }
    pub(super) fn unused_done(&self) -> bool {
        self.runtime.is_some() || self.unused == 127
    }
    pub(super) fn enabled(&self, a: HandoffAction) -> bool {
        self.actions().contains(&a)
    }
    fn receipt(&mut self) {
        let mut runtime = Registration::new(1, self.binding);
        for a in [C::Write, C::Prefix, C::Publish]
            .into_iter()
            .take(usize::from(self.sdk))
        {
            runtime.apply(R::Product(a));
        }
        runtime.apply(R::Product(C::Admit(0)));
        // A remembered deferred decision is installed before the new resolver
        // becomes visible to registration, copy, claim or worker wake.
        if self.decision != Decision::Pending && self.fault != HandoffFault::RetryLosesDecision {
            for a in [
                C::BeginControl(0, self.decision),
                C::ResolveControl(0),
                C::EndControl(0, true),
            ] {
                runtime.apply(R::Product(a));
            }
        }
        self.runtime = Some(runtime);
    }
    pub(super) fn runtime_observation(&mut self, next: Registration) {
        if next.inner.completion.stage(0) >= ReceiptStage::Acquired && !self.admission.collected {
            self.admission.apply(A::Collect);
        }
        if self.decision == Decision::Pending {
            self.decision = next.inner.completion.decision(0);
        }
        self.runtime = Some(next);
    }
    fn call_actions(&self) -> Vec<HandoffAction> {
        let mut a = Vec::new();
        for k in self.call.actions() {
            let enabled = match k {
                K::Enter
                | K::CloseGate
                | K::SettleGate
                | K::ReadResult
                | K::Admit(_)
                | K::Begin(_)
                | K::ReturnCallback(_)
                | K::SettleCallback(_)
                | K::ReleaseEnvironment => true,
                K::ReturnNative => self.started,
                K::FinishContinuation => self.handed,
                _ => false,
            };
            if enabled {
                a.push(HandoffAction::Call(k));
            }
        }
        a
    }
    fn call_release(&mut self) {
        // This finite profile has no escaped managed result. Execute actual Call
        // transitions together; the separate model explores their permutations.
        for a in [
            K::RetireLedger,
            K::DropAlias,
            K::ReleaseBorrower,
            K::CloseHandle,
            K::CollectValue,
        ] {
            self.call.apply(a);
        }
    }
}

impl Protocol for Handoff {
    type Action = HandoffAction;
    fn name(&self) -> String {
        format!("call-admission-wake-{:?}-full{}", self.binding, self.full)
    }
    fn actions(&self) -> Vec<Self::Action> {
        use HandoffAction as H;
        if self.disposed {
            return vec![];
        }
        let mut a = self.call_actions();
        if self.call.stage == Stage::Entered && !self.started {
            a.push(H::StartSdk);
        }
        if self.call.stage == Stage::Read && !self.handed {
            a.push(H::HandOff);
        }
        if !self.admission.attempted
            && (self.handed || (self.fault == HandoffFault::AdmitBeforeRead && self.started))
        {
            a.push(H::TryAdmit);
        }
        if self.handed && !self.admission.credit_available {
            a.push(H::Credit);
        }
        if self.admission.owner == Owner::Deferred
            && self.admission.credit_available
            && self.sdk < 3
        {
            a.push(H::Retry);
        }
        if self.handed
            && self.admission.attempted
            && self.runtime.is_none()
            && self.decision == Decision::Pending
            && self.sdk < 3
        {
            a.extend([
                H::Control(Decision::Cancelled),
                H::Control(Decision::TimedOut),
            ]);
        }
        if self.started {
            match self.sdk {
                0 => a.push(H::SdkWrite),
                1 => a.push(H::SdkPrefix),
                2 => a.push(H::SdkPublish),
                _ => {}
            }
        }
        if self.admission.owner == Owner::Deferred && self.sdk == 3 && !self.admission.collected {
            a.push(H::DirectCopy);
        }
        if self.admission.collected && self.admission.owner != Owner::Released {
            a.push(H::ReleaseDrain);
        }
        let physical = self.admission.collected;
        let carrier_safe = self
            .runtime
            .as_ref()
            .is_none_or(|r| r.inner.completion.retired());
        if self.pin
            && ((physical && carrier_safe)
                || (self.fault == HandoffFault::ReleasePinWithCall && !self.call.environment))
        {
            a.push(H::ReleasePin);
        }
        if self.runtime.is_none() && self.admission.collected {
            for (bit, requires) in [(1, 0), (2, 1), (4, 2), (8, 2), (16, 4), (32, 20), (64, 2)] {
                if self.unused & bit == 0 && self.unused & requires == requires {
                    a.push(H::Unused(bit));
                }
            }
        }
        if let Some(r) = &self.runtime {
            for action in r.actions() {
                if matches!(
                    action,
                    R::Product(C::Admit(_) | C::Write | C::Prefix | C::Publish)
                ) {
                    continue;
                }
                if matches!(action, R::Product(C::BeginControl(..)))
                    && matches!(self.decision, Decision::Cancelled | Decision::TimedOut)
                {
                    continue;
                }
                if matches!(action, R::Product(C::Release(_)))
                    && self.admission.owner != Owner::Released
                {
                    continue;
                }
                a.push(H::Runtime(action));
            }
        }
        if (!self.call.environment
            && !self.pin
            && self.admission.root_released
            && self.runtime.as_ref().is_none_or(Protocol::terminal)
            && self.unused_done())
            || (self.fault == HandoffFault::DisposeOnCancel && self.decision == Decision::Cancelled)
        {
            a.push(H::Dispose);
        }
        a
    }
    fn apply(&mut self, a: Self::Action) {
        use HandoffAction as H;
        match a {
            H::Call(k) => {
                self.call.apply(k);
                if k == K::ReturnNative && self.fault == HandoffFault::ReturnPublishesSdk {
                    self.sdk = 3;
                    self.admission.apply(A::Publish);
                }
                if k == K::ReleaseEnvironment {
                    self.call_release();
                }
            }
            H::StartSdk => {
                self.started = true;
                self.pin = true;
            }
            H::HandOff => self.handed = true,
            H::TryAdmit => {
                self.admission.apply(A::TryAdmit);
                if self.admission.owner == Owner::Receipt {
                    self.receipt();
                }
                if self.fault == HandoffFault::LoseDeferredDrain
                    && self.admission.owner == Owner::Deferred
                {
                    self.admission.owner = Owner::Lost;
                }
            }
            H::Credit => self.admission.apply(A::CreditAvailable),
            H::Retry => {
                self.admission.apply(A::RetryAdmit);
                self.receipt();
            }
            H::Control(d) => {
                self.decision = d;
                self.admission.apply(A::Cancel);
            }
            H::SdkWrite | H::SdkPrefix | H::SdkPublish => {
                let c = match a {
                    H::SdkWrite => C::Write,
                    H::SdkPrefix => C::Prefix,
                    _ => C::Publish,
                };
                self.sdk += 1;
                if let Some(r) = &mut self.runtime {
                    r.apply(R::Product(c));
                }
                if self.sdk == 3 {
                    self.admission.apply(A::Publish);
                }
            }
            H::DirectCopy => {
                self.admission.apply(A::CollectDeferred);
                if self.decision == Decision::Pending {
                    self.decision = Decision::Completed;
                }
            }
            H::ReleaseDrain => {
                self.admission.apply(A::Release);
                self.admission.apply(A::ReleaseRoot);
            }
            H::ReleasePin => {
                self.pin = false;
            }
            H::Runtime(r) => {
                let mut next = self.runtime.clone().unwrap();
                next.apply(r);
                self.runtime_observation(next);
            }
            H::Unused(bit) => self.unused |= bit,
            H::Dispose => self.disposed = true,
        }
    }
    fn check(&self) -> Result<(), Violation> {
        self.call.check()?;
        self.admission.check()?;
        if let Some(r) = &self.runtime {
            r.check()?;
            require(
                r.inner.completion.published() == (self.sdk == 3),
                "NL-I2",
                "SDK publication disagrees across handoff",
            )?;
            if matches!(self.decision, Decision::Cancelled | Decision::TimedOut) {
                require(
                    r.inner.completion.decision(0) == self.decision,
                    "NL-I1",
                    "accepted deferred decision lost at resolver transfer",
                )?;
            }
        }
        require(
            !self.admission.attempted || self.handed,
            "NL-I9",
            "admission bypassed result-read/root handoff",
        )?;
        require(
            !self.started || self.admission.collected || self.pin,
            "NL-I5",
            "Call release destroyed an outstanding SDK carrier pin",
        )?;
        require(
            !self.disposed
                || (!self.call.environment
                    && !self.pin
                    && self.admission.root_released
                    && self.runtime.as_ref().is_none_or(Protocol::terminal)
                    && self.unused_done()),
            "NL-I5",
            "Owner disposed before Call/drain/wake quiescence",
        )?;
        Ok(())
    }
    fn check_step(&self, a: Self::Action, after: &Self) -> Result<(), Violation> {
        use HandoffAction as H;
        match a {
            H::Call(k) => {
                self.call.check_step(k, &after.call)?;
                require(
                    self.sdk == after.sdk,
                    "NL-I3",
                    "physical Call return fabricated SDK publication",
                )?;
            }
            H::Runtime(r) => self
                .runtime
                .as_ref()
                .unwrap()
                .check_step(r, after.runtime.as_ref().unwrap())?,
            H::Control(_) => {
                require(
                    self.sdk == after.sdk
                        && self.pin == after.pin
                        && self.call.environment == after.call.environment
                        && self.admission.collected == after.admission.collected,
                    "NL-I3",
                    "logical control fabricated physical release",
                )?;
            }
            _ => {}
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.disposed
    }
    fn witnesses(&self) -> Vec<&'static str> {
        let mut w = Vec::new();
        if self.sdk == 3 && self.call.stage == Stage::Entered {
            w.push("sdk-publication-before-call-return");
        }
        if !self.call.environment && self.sdk < 3 && self.pin {
            w.push("call-environment-released-sdk-pin-retained");
        }
        if self.call.stage == Stage::Closed
            && self
                .call
                .callbacks
                .iter()
                .any(|c| !matches!(c, Callback::Unused | Callback::Settled))
        {
            w.push("call-close-waits-for-callback");
        }
        if self.admission.owner == Owner::Deferred
            && self.decision != Decision::Pending
            && self.sdk < 3
        {
            w.push("controlled-deferred-sdk-still-running");
        }
        if self.runtime.is_some() && self.admission.cancelled {
            w.push("deferred-control-transferred-before-reactor-publication");
        }
        if self.runtime.is_none() && self.admission.collected {
            w.push("direct-deferred-drain-without-normal-credit");
        }
        w
    }
}
