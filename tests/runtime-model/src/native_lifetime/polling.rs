//! One real v1 polling Call shares the C helper and Owner with notified Calls.
use super::admission::{Action as A, Owner};
use super::call::Action as K;
use super::joined::Aux;
use super::search::{Violation, require};
use super::{Admission, Binding, Call, Protocol};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollingAction {
    Call(K),
    Start,
    Handoff,
    Admit,
    Bind,
    Publish,
    Copy,
    Claim,
    Physical,
    WorkerBegin,
    WorkerEnd,
    Settle,
    DrainRelease,
    Release,
    Close,
    Retire,
}
const CALL: [Option<K>; 13] = [
    Some(K::Enter),
    None,
    Some(K::Admit(0)),
    Some(K::Begin(0)),
    Some(K::ReturnCallback(0)),
    Some(K::SettleCallback(0)),
    Some(K::ReturnNative),
    Some(K::CloseGate),
    Some(K::SettleGate),
    Some(K::ReadResult),
    None,
    Some(K::FinishContinuation),
    Some(K::ReleaseEnvironment),
];
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "SDK pin and physical publication are independent"
)]
pub(super) struct Polling {
    pub binding: Binding,
    pub worker: u8,
    pub call: Call,
    pub drain: Admission,
    pub stage: Aux,
    pub pc: usize,
    pub bound: bool,
    pub published: bool,
    pub closed: bool,
    pub retired: bool,
}
impl Polling {
    pub fn new() -> Self {
        Self::with_binding(Binding::CJoined)
    }
    pub fn with_binding(binding: Binding) -> Self {
        Self {
            binding,
            worker: if binding == Binding::RustChannel {
                0
            } else {
                2
            },
            call: Call::new(1, false),
            drain: Admission::new(false),
            stage: Aux::Absent,
            pc: 0,
            bound: false,
            published: false,
            closed: false,
            retired: false,
        }
    }
    pub fn started(&self) -> bool {
        self.pc > 0
    }
    pub fn physical(&self) -> bool {
        self.stage >= Aux::Physical
    }
    pub fn drained(&self) -> bool {
        self.stage >= Aux::Settled && self.retired
    }
    pub fn released(&self) -> bool {
        self.stage == Aux::Released && self.retired
    }
}
impl Protocol for Polling {
    type Action = PollingAction;
    fn name(&self) -> String {
        "multiple-call-v1-polling".into()
    }
    fn actions(&self) -> Vec<PollingAction> {
        use PollingAction as P;
        if self.pc < CALL.len() {
            return match (self.pc, CALL[self.pc]) {
                (1, _) => vec![P::Start],
                (10, _) => vec![P::Handoff],
                (_, Some(k)) if self.call.actions().contains(&k) => vec![P::Call(k)],
                _ => vec![],
            };
        }
        let mut a = Vec::new();
        match self.stage {
            Aux::Absent => a.push(P::Admit),
            Aux::Waiting if !self.bound => a.push(P::Bind),
            Aux::Waiting if !self.published => a.push(P::Publish),
            Aux::Waiting => a.push(P::Copy),
            Aux::Copied => a.push(P::Claim),
            Aux::Claimed => a.push(P::Physical),
            Aux::Physical => a.push(P::Settle),
            Aux::Settled if self.drain.owner == Owner::Released && self.worker == 2 => {
                a.push(P::Release);
            }
            Aux::Settled | Aux::Released => {}
        }
        if self.physical() && self.drain.owner != Owner::Released {
            a.push(P::DrainRelease);
        }
        if self.binding == Binding::RustChannel && self.physical() && self.worker == 0 {
            a.push(P::WorkerBegin);
        }
        if self.worker == 1 {
            a.push(P::WorkerEnd);
        }
        if self.physical() && !self.closed {
            a.push(P::Close);
        }
        if self.closed && !self.retired {
            a.push(P::Retire);
        }
        a
    }
    fn apply(&mut self, a: PollingAction) {
        use PollingAction as P;
        match a {
            P::Call(k) => {
                self.call.apply(k);
                self.pc += 1;
                if k == K::ReleaseEnvironment {
                    for k in [
                        K::RetireLedger,
                        K::DropAlias,
                        K::ReleaseBorrower,
                        K::CloseHandle,
                        K::CollectValue,
                    ] {
                        self.call.apply(k);
                    }
                }
            }
            P::Start | P::Handoff => self.pc += 1,
            P::Admit => {
                self.drain.apply(A::TryAdmit);
                self.stage = Aux::Waiting;
            }
            P::Bind => self.bound = true,
            P::Publish => {
                self.published = true;
                self.drain.apply(A::Publish);
            }
            P::Copy => self.stage = Aux::Copied,
            P::Claim => self.stage = Aux::Claimed,
            P::Physical => {
                self.stage = Aux::Physical;
                self.drain.apply(A::Collect);
            }
            P::WorkerBegin => self.worker = 1,
            P::WorkerEnd => self.worker = 2,
            P::Settle => self.stage = Aux::Settled,
            P::Release => self.stage = Aux::Released,
            P::DrainRelease => {
                self.drain.apply(A::Release);
                self.drain.apply(A::ReleaseRoot);
            }
            P::Close => self.closed = true,
            P::Retire => self.retired = true,
        }
    }
    fn check(&self) -> Result<(), Violation> {
        self.call.check()?;
        require(
            self.stage != Aux::Released || self.worker == 2,
            "NL-I5",
            "polling record freed during worker wake",
        )?;
        self.drain.check()?;
        require(
            self.stage != Aux::Released || self.drain.owner == Owner::Released,
            "NL-I9",
            "polling record released before drain and root return",
        )?;
        require(
            !self.retired || self.physical(),
            "NL-I4",
            "polling storage retired before physical copy",
        )?;
        require(
            self.stage < Aux::Copied || self.bound && self.published,
            "NL-I2",
            "polling copy before acquired publication",
        )?;
        require(
            self.drain.owner != Owner::Released || self.physical(),
            "NL-I9",
            "polling drain lost before copy",
        )
    }
    fn check_step(&self, a: PollingAction, n: &Self) -> Result<(), Violation> {
        if let PollingAction::Call(k) = a {
            self.call.check_step(k, &n.call)?;
        }
        Ok(())
    }
    fn terminal(&self) -> bool {
        self.released()
    }
}
