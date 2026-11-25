use super::admission::{Action as AdmissionAction, Owner};
use super::search::{Violation, require};
use super::{Admission, Decision, Protocol};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Poll {
    Admitted,
    Ready,
    Selected,
    Settled,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Retry {
    Admitted(usize),
    Registered(usize),
    Physical(usize),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum AdmissionPolicy {
    DirectOnly,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferredPollAction {
    RejectAdmission(usize),
    Control(usize, Decision),
    PublishSdk,
    PollReady,
    SelectPoll,
    SettlePoll,
    Retry(usize),
    RegisterRetry(usize),
    CopyReceipt(usize),
    SettleReceipt(usize),
    ReturnSdk,
    DirectCopy(usize),
    RetireCarrier,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeferredPoll {
    poll: Poll,
    poll_credit: bool,
    drains: [Admission; 2],
    decisions: [Decision; 2],
    policy: AdmissionPolicy,
    retry: Option<Retry>,
    sdk_reference: bool,
    retired: bool,
}

impl Default for DeferredPoll {
    fn default() -> Self {
        Self {
            poll: Poll::Admitted,
            poll_credit: true,
            drains: [Admission::new(true), Admission::new(true)],
            decisions: [Decision::Pending; 2],
            policy: AdmissionPolicy::DirectOnly,
            retry: None,
            sdk_reference: false,
            retired: false,
        }
    }
}

impl DeferredPoll {
    #[must_use]
    pub fn allowing_retry() -> Self {
        Self {
            policy: AdmissionPolicy::Retry,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn sdk_has_reference(&self) -> bool {
        self.sdk_reference
    }

    /// # Panics
    /// Requires a deferred receipt index of zero or one.
    #[must_use]
    pub fn decision(&self, index: usize) -> Decision {
        self.decisions[index]
    }

    #[must_use]
    pub fn poll_has_credit(&self) -> bool {
        self.poll_credit
    }
}

impl Protocol for DeferredPoll {
    type Action = DeferredPollAction;

    fn name(&self) -> String {
        match self.policy {
            AdmissionPolicy::DirectOnly => "source-poll-and-two-direct-deferred-receipts",
            AdmissionPolicy::Retry => "source-poll-and-one-deferred-retry",
        }
        .into()
    }

    fn actions(&self) -> Vec<Self::Action> {
        use DeferredPollAction as A;
        let mut a = Vec::new();
        for (index, drain) in self.drains.iter().enumerate() {
            if !drain.attempted {
                a.push(A::RejectAdmission(index));
            }
            if drain.owner == Owner::Deferred
                && !drain.published
                && self.decisions[index] == Decision::Pending
            {
                for decision in [Decision::Cancelled, Decision::TimedOut] {
                    a.push(A::Control(index, decision));
                }
            }
        }
        if self.drains.iter().all(|drain| drain.attempted) {
            if !self.drains[0].published {
                a.push(A::PublishSdk);
            }
            if self.poll == Poll::Admitted {
                a.push(A::PollReady);
            }
            let deferred = self
                .drains
                .iter()
                .position(|drain| drain.owner == Owner::Deferred);
            if self.poll == Poll::Selected {
                a.push(A::SettlePoll);
            } else {
                let direct = deferred.filter(|&i| self.drains[i].published);
                if let Some(index) = direct {
                    a.push(A::DirectCopy(index));
                } else if self.poll == Poll::Ready {
                    a.push(A::SelectPoll);
                } else if self.policy == AdmissionPolicy::Retry
                    && self.poll == Poll::Settled
                    && self.retry.is_none()
                    && let Some(i) = deferred
                {
                    a.push(A::Retry(i));
                }
                match self.retry {
                    Some(Retry::Admitted(i)) => a.push(A::RegisterRetry(i)),
                    Some(Retry::Registered(i)) if self.drains[i].published => {
                        a.push(A::CopyReceipt(i));
                    }
                    Some(Retry::Physical(i)) if direct.is_none() => a.push(A::SettleReceipt(i)),
                    _ => {}
                }
            }
        }
        if self.sdk_reference {
            a.push(A::ReturnSdk);
        }
        if !self.retired && self.drains.iter().all(Protocol::terminal) {
            a.push(A::RetireCarrier);
        }
        a
    }

    fn apply(&mut self, action: Self::Action) {
        use DeferredPollAction as A;
        match action {
            A::RejectAdmission(i) => self.drains[i].apply(AdmissionAction::TryAdmit),
            A::Control(i, decision) => {
                self.drains[i].apply(AdmissionAction::Cancel);
                self.decisions[i] = decision;
            }
            A::PublishSdk => {
                self.sdk_reference = matches!(self.retry, Some(Retry::Registered(_)));
                for drain in &mut self.drains {
                    drain.apply(AdmissionAction::Publish);
                }
            }
            A::PollReady => self.poll = Poll::Ready,
            A::SelectPoll => self.poll = Poll::Selected,
            A::SettlePoll => {
                self.poll = Poll::Settled;
                self.poll_credit = false;
            }
            A::Retry(i) => {
                let drain = &mut self.drains[i];
                drain.apply(AdmissionAction::CreditAvailable);
                drain.apply(AdmissionAction::RetryAdmit);
                self.retry = Some(Retry::Admitted(i));
            }
            A::RegisterRetry(i) => self.retry = Some(Retry::Registered(i)),
            A::CopyReceipt(i) => {
                self.drains[i].apply(AdmissionAction::Collect);
                self.retry = Some(Retry::Physical(i));
            }
            A::SettleReceipt(i) => {
                for a in [AdmissionAction::Release, AdmissionAction::ReleaseRoot] {
                    self.drains[i].apply(a);
                }
                if self.decisions[i] == Decision::Pending {
                    self.decisions[i] = Decision::Completed;
                }
                self.retry = None;
            }
            A::ReturnSdk => self.sdk_reference = false,
            A::DirectCopy(i) => {
                for a in [
                    AdmissionAction::CollectDeferred,
                    AdmissionAction::Release,
                    AdmissionAction::ReleaseRoot,
                ] {
                    self.drains[i].apply(a);
                }
                if self.decisions[i] == Decision::Pending {
                    self.decisions[i] = Decision::Completed;
                }
            }
            A::RetireCarrier => self.retired = true,
        }
    }

    fn check(&self) -> Result<(), Violation> {
        require(
            self.poll_credit == (self.poll != Poll::Settled),
            "NL-I9",
            "polling credit changed outside its settlement",
        )?;
        require(
            !self.retired || self.drains.iter().all(Protocol::terminal),
            "NL-I4",
            "retired a shared carrier with a live Deferred drain",
        )?;
        for (i, drain) in self.drains.iter().enumerate() {
            drain.check()?;
            let has_credit = matches!(
                self.retry,
                Some(Retry::Admitted(g) | Retry::Registered(g) | Retry::Physical(g)) if i == g
            );
            require(
                (drain.owner == Owner::Receipt) == has_credit && (!has_credit || !self.poll_credit),
                "NL-I9",
                "retry consumed the still occupied polling credit or direct copy acquired a credit",
            )?;
        }
        Ok(())
    }

    fn check_step(&self, action: Self::Action, after: &Self) -> Result<(), Violation> {
        if let DeferredPollAction::DirectCopy(i) | DeferredPollAction::SettleReceipt(i) = action {
            require(
                self.poll == after.poll
                    && self.poll_credit == after.poll_credit
                    && self.drains[1 - i] == after.drains[1 - i],
                "NL-I9",
                "Deferred settlement affected the polling credit or another receipt",
            )?;
            require(
                self.decisions[i] == Decision::Pending || self.decisions[i] == after.decisions[i],
                "NL-I3",
                "Deferred completion overwrote an accepted decision",
            )?;
        }
        Ok(())
    }

    fn terminal(&self) -> bool {
        self.retired && self.poll == Poll::Settled && !self.sdk_reference
    }

    fn witnesses(&self) -> Vec<&'static str> {
        let mut result = Vec::new();
        if self.poll == Poll::Selected && self.drains[0].published {
            result.push("sdk-publication-after-poll-selection");
        }
        if self.poll_credit && self.drains.iter().all(Protocol::terminal) {
            result.push("two-direct-copies-with-poll-credit-held");
        }
        if let Some(Retry::Admitted(i)) = self.retry
            && self.drains[i].published
        {
            result.push("published-after-retry-before-registration");
        }
        if self.sdk_reference && self.drains.iter().any(|d| d.owner == Owner::Receipt) {
            result.push("retry-registration-transferred-the-sdk-reference");
        }
        result
    }
}
