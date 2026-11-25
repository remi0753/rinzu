use super::{Binding, Domain, ReplayReport, SharedTopology, replay_shared};
use std::collections::{HashMap, HashSet};

#[path = "domain_chain_replay.rs"]
mod chain;
pub use chain::{DomainChainReplayReport, replay_consecutive_domains, replay_domain_chain};
#[path = "joined_domain_replay.rs"]
mod joined;
pub use joined::{JoinedDomainReplayReport, replay_joined_handoff};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum EvidenceProfile {
    #[default]
    Coalesced,
    Separate,
    Consecutive,
}

impl EvidenceProfile {
    fn versions(self) -> &'static [i64] {
        match self {
            Self::Coalesced => &[3, 2],
            Self::Separate => &[3, 3],
            Self::Consecutive => &[3, 3, 2],
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainReplayReport {
    pub local: [ReplayReport; 2],
    pub concurrent_notifications: bool,
    pub free_during_foreign_notification: bool,
    pub notify_after_environment_free: bool,
    pub settlement_order: Vec<(Domain, usize)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainReuseReplayReport {
    pub domains: DomainReplayReport,
    pub first_target_destroyed: Domain,
    pub origin_environment_released_before_handoff: bool,
    pub old_notified_while_replacement_unpublished: bool,
    pub old_returned_while_replacement_unpublished: bool,
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Observed reference, notification and destruction boundaries are independent"
)]
struct Evidence {
    rows: Vec<String>,
    carrier: [Option<i64>; 3],
    receipts: HashMap<u64, usize>,
    bound_carriers: HashMap<u64, i64>,
    bound_wakes: HashSet<u64>,
    registered: HashSet<u64>,
    exchanges: HashSet<u64>,
    captures: HashSet<u64>,
    counts: HashSet<u64>,
    returned: HashSet<u64>,
    context: Option<i64>,
    context_observed: bool,
    context_touches: usize,
    context_freed: bool,
    target: Option<i64>,
    runtime_count: bool,
    target_freed: bool,
    notified: bool,
    notifying: bool,
    attached_notifications: usize,
    sdk_progress: usize,
    profile: EvidenceProfile,
}

fn member(id: u64) -> Result<usize, String> {
    match id {
        100 => Ok(0),
        101 => Ok(1),
        102 => Ok(2),
        _ => Err(format!("unknown batch member {id}")),
    }
}

fn address(value: i64) -> bool {
    value > 1 && value % 8 == 0
}

impl Evidence {
    fn members(&self) -> usize {
        self.profile.versions().len()
    }

    fn sdk_references(&self) -> usize {
        self.captures.difference(&self.returned).count()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Identity and reference observations are checked at their trace boundaries"
    )]
    fn observe(&mut self, kind: &str, id: u64, a: i64, b: i64) -> Result<(), String> {
        let valid = match kind {
            "domain_context" => {
                id == 0 && b == 0 && address(a) && self.context.replace(a).is_none()
            }
            "context_identity" => {
                id == 0
                    && b == 0
                    && self.context == Some(a)
                    && !self.context_freed
                    && !std::mem::replace(&mut self.context_observed, true)
            }
            "environment_identity_free" => {
                id == 0
                    && b == 0
                    && self.context == Some(a)
                    && !std::mem::replace(&mut self.context_freed, true)
            }
            "carrier_address" => {
                let g = member(id)?;
                self.profile.versions().get(g) == Some(&b)
                    && address(a)
                    && self.carrier[g].replace(a).is_none()
            }
            "receipt_carrier" => {
                b == 0
                    && self.carrier.contains(&Some(a))
                    && self.bound_carriers.insert(id, a).is_none()
            }
            "wake_bind" => {
                if !self.receipts.contains_key(&id)
                    || !self.bound_wakes.insert(id)
                    || !address(a)
                    || b != 0
                    || self.target_freed
                    || self.target.is_some_and(|target| target != a)
                {
                    false
                } else {
                    self.target = Some(a);
                    true
                }
            }
            "batch_target" => {
                member(id)?;
                b == 0
                    && self.target == Some(a)
                    && !self.target_freed
                    && self.exchanges.contains(&id)
                    && self.captures.insert(id)
            }
            "producer_refs" => {
                let g = member(id)?;
                let valid_count = match self.profile {
                    EvidenceProfile::Coalesced if g < 2 => {
                        b == [1, 3][g] && (g == 1 || self.context_freed)
                    }
                    EvidenceProfile::Consecutive if g == 2 => b == 4 && !self.context_freed,
                    EvidenceProfile::Separate | EvidenceProfile::Consecutive => {
                        self.context_freed && usize::try_from(b).ok() == Some(self.sdk_references())
                    }
                    EvidenceProfile::Coalesced => false,
                };
                valid_count
                    && self.target == Some(a)
                    && !self.target_freed
                    && self.captures.len() == self.members()
                    && self.counts.insert(id)
            }
            "runtime_refs" => {
                let valid_count = if self.profile == EvidenceProfile::Coalesced {
                    b == 2 && self.counts.contains(&101)
                } else {
                    b == 3 && self.counts.len() == self.members() - 2
                };
                id == 0
                    && valid_count
                    && self.target == Some(a)
                    && !self.target_freed
                    && !std::mem::replace(&mut self.runtime_count, true)
            }
            "target_identity_free" => {
                id == 0
                    && b == 0
                    && self.target == Some(a)
                    && self.counts.len() == self.members()
                    && self.context_freed
                    && !std::mem::replace(&mut self.target_freed, true)
            }
            "sdk_progress" => {
                self.sdk_progress += 1;
                id == 0 && b == 0 && self.target == Some(a) && !self.context_freed && self.notified
            }
            "trace_progress_refs" => {
                id == 0 && b > 0 && self.target == Some(a) && !self.context_freed
            }
            _ => {
                match kind {
                    "receipt" => {
                        let g =
                            member(u64::try_from(a).map_err(|_| "invalid receipt generation")?)?;
                        if self.bound_carriers.get(&id).copied() != self.carrier[g]
                            || self.carrier[g].is_none()
                            || self.receipts.insert(id, g).is_some()
                        {
                            return Err("receipt associated with a foreign carrier".into());
                        }
                    }
                    "sdk_exchange" if a == 1 => {
                        self.exchanges.insert(id);
                    }
                    "registered" if a == 1 => {
                        if !self.bound_wakes.contains(&id) || !self.registered.insert(id) {
                            return Err("registration lacks its Domain target".into());
                        }
                    }
                    "merge_wake" if self.captures.len() != self.members() => {
                        return Err("merge lacks the actual captures".into());
                    }
                    "notify_enter" => {
                        if !self.captures.contains(&id) || self.target_freed {
                            return Err("notification has no live local capture".into());
                        }
                        self.notified = true;
                        self.notifying = true;
                        self.attached_notifications += usize::from(a == 1);
                    }
                    "notify_leave" => self.notifying = false,
                    "context_touch" => {
                        if !std::mem::take(&mut self.context_observed) || self.context_freed {
                            return Err("context access lacks its live Domain identity".into());
                        }
                        self.context_touches += 1;
                    }
                    "producer_release" if !self.counts.contains(&id) => {
                        return Err("SDK return lacks target reference count".into());
                    }
                    "producer_released" => {
                        self.returned.insert(id);
                    }
                    "runtime_reference_release" if !self.runtime_count => {
                        return Err("runtime return lacks target reference count".into());
                    }
                    "target_free" if !self.target_freed => {
                        return Err("target destruction lacks identity".into());
                    }
                    "env_free" if !self.context_freed => {
                        return Err("environment destruction lacks identity".into());
                    }
                    _ => {}
                }
                self.rows.push(format!("{kind},{id},{a},{b}"));
                return Ok(());
            }
        };
        if valid {
            Ok(())
        } else {
            Err(format!("invalid Domain evidence {kind},{id},{a},{b}"))
        }
    }

    fn complete(&self, binding: Binding) -> bool {
        self.carrier[..self.members()].iter().all(Option::is_some)
            && self.context_freed
            && !self.context_observed
            && self.context_touches
                == if binding == Binding::RustChannel {
                    self.members()
                } else {
                    self.attached_notifications
                }
            && self.target_freed
            && self.bound_wakes.len() == self.members()
            && self.captures.len() == self.members()
            && self.counts.len() == self.members()
    }
}

#[derive(Clone, Copy)]
struct StorageHandoff {
    address: i64,
    environment_released: bool,
    alias_retained: bool,
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Receipt retirement, record release and notification timing are independent observations"
)]
struct ReuseEvidence {
    handoff: Option<StorageHandoff>,
    old_settled: Option<u64>,
    old_retired: bool,
    old_record_released: bool,
    alias_seen: bool,
    open_control: Option<u8>,
    rejected_controls: u8,
    new_sample: Option<i64>,
    new_checks: usize,
    first_target_destroyed: Option<Domain>,
    retired_replacement_checked: bool,
    notified_before_environment_free: bool,
    notified_while_unpublished: bool,
    returned_while_unpublished: bool,
}

impl ReuseEvidence {
    fn stale_receipt_checked(&self) -> bool {
        self.handoff.is_some_and(|handoff| {
            if handoff.alias_retained {
                self.alias_seen && self.rejected_controls == 3
            } else {
                !self.alias_seen && self.rejected_controls == 0
            }
        })
    }

    /// Returns true for evidence that is checked here and needs no local projection.
    #[allow(
        clippy::too_many_lines,
        reason = "Cross-Domain identity, order and snapshot checks share the same timeline"
    )]
    fn observe(
        &mut self,
        domains: &[Evidence; 2],
        domain: Domain,
        kind: &str,
        id: u64,
        a: i64,
        b: i64,
    ) -> Result<bool, String> {
        let [old, new] = domains;
        let handed_off = self.handoff.is_some();
        let new_live =
            handed_off && new.registered.len() == 2 && !new.context_freed && !new.target_freed;
        let new_destroyed_first = self.first_target_destroyed == Some(Domain::Second);
        let unpublished = new.exchanges.is_empty();
        let valid = match (domain, kind) {
            (Domain::Second, "storage_handoff") => {
                if id != 100
                    || b != 0
                    || handed_off
                    || !self.old_retired
                    || old.carrier[0] != Some(a)
                    || old.captures.len() != 2
                    || !old.returned.contains(&101)
                    || old.notified
                    || old.counts.contains(&100)
                    || new.carrier.iter().any(Option::is_some)
                    || new.context.is_some()
                {
                    return Err(
                        "handoff lacks a retired carrier and a retained old SDK target".into(),
                    );
                }
                self.handoff = Some(StorageHandoff {
                    address: a,
                    environment_released: old.context_freed,
                    alias_retained: !self.old_record_released,
                });
                return Ok(true);
            }
            (Domain::Second, "carrier_address")
                if !handed_off || (id == 100 && Some(a) != self.handoff.map(|h| h.address)) =>
            {
                return Err("replacement carrier did not use the handed-off allocation".into());
            }
            (Domain::Second, "domain_context") if !handed_off => {
                return Err("replacement context appeared before storage handoff".into());
            }
            (Domain::First, "owner_settle") if old.receipts.get(&id) == Some(&0) => {
                self.old_settled = Some(id);
                return Ok(false);
            }
            (Domain::First, "carrier_retire") if id == 100 => {
                if self.old_settled.is_none()
                    || handed_off
                    || old.carrier[0] != Some(a)
                    || self.old_retired
                    || b != 0
                {
                    return Err("old generation retired without its settled receipt".into());
                }
                self.old_retired = true;
                return Ok(false);
            }
            (_, "carrier_retire") => {
                if domains[domain.index()].carrier[member(id)?] != Some(a) || b != 0 {
                    return Err("retirement identified the wrong storage".into());
                }
                return Ok(false);
            }
            (Domain::First, "old_receipt") => {
                new_live
                    && unpublished
                    && !self.old_record_released
                    && self.old_settled == Some(id)
                    && a == 7183
                    && b == 0
                    && self.new_checks == 1
                    && !std::mem::replace(&mut self.alias_seen, true)
            }
            (Domain::First, "control_begin" | "control_end") if self.old_settled == Some(id) => {
                if !new_live
                    || !unpublished
                    || self.old_record_released
                    || b != 0
                    || self.new_checks != 1
                {
                    return Err("stale control lacked a live replacement and old receipt".into());
                }
                if kind == "control_begin"
                    && let Ok(reason @ (1 | 2)) = u8::try_from(a)
                {
                    self.rejected_controls & reason == 0
                        && self.open_control.replace(reason).is_none()
                } else if kind == "control_end" && a == 0 {
                    self.open_control.take().is_some_and(|reason| {
                        self.rejected_controls |= reason;
                        true
                    })
                } else {
                    false
                }
            }
            (Domain::First, "record_release") if self.old_settled == Some(id) => {
                if !self.old_retired || (handed_off && !self.stale_receipt_checked()) {
                    return Err(
                        "old record released before its cached read and stale controls".into(),
                    );
                }
                self.old_record_released = true;
                return Ok(false);
            }
            (Domain::First, "notify_enter" | "producer_released") if id == 100 => {
                let replacement_finished =
                    new_destroyed_first && new.target_freed && new.returned.contains(&100);
                if !(new_live || replacement_finished) || !self.stale_receipt_checked() {
                    return Err("old SDK action lacked a registered replacement lifetime".into());
                }
                if kind == "notify_enter" {
                    self.notified_before_environment_free = !old.context_freed;
                    self.notified_while_unpublished = unpublished;
                } else {
                    self.returned_while_unpublished = unpublished;
                }
                return Ok(false);
            }
            (_, "target_identity_free") if self.first_target_destroyed.is_none() => {
                if domain == Domain::Second && (old.notified || self.new_checks != 3) {
                    return Err("replacement destruction preceded its last live snapshot".into());
                }
                self.first_target_destroyed = Some(domain);
                return Ok(false);
            }
            (Domain::Second, "replacement_refs" | "trace_progress_refs") => {
                new_live
                    && id == 0
                    && new.target == Some(a)
                    && (unpublished && b == 3
                        || new.exchanges.len() == 2 && new.returned.contains(&101) && b == 2)
                    && self.new_sample.replace(b).is_none()
            }
            (Domain::Second, "replacement_check") => {
                let phase = match self.new_checks {
                    0 => unpublished && !self.alias_seen && !old.notified,
                    1 => {
                        unpublished
                            && self.stale_receipt_checked()
                            && self.handoff.is_some_and(|handoff| {
                                handoff.alias_retained || old.notified && !old.counts.contains(&100)
                            })
                    }
                    2 => {
                        old.target_freed && old.returned.contains(&100)
                            || !unpublished && !old.notified
                    }
                    _ => false,
                };
                new_live
                    && phase
                    && self.new_sample.take().is_some()
                    && id == 100
                    && b == 0
                    && (unpublished && new.target == Some(a) || !unpublished && a == 1)
                    && {
                        self.new_checks += 1;
                        true
                    }
            }
            (Domain::Second, "replacement_retired_check") => {
                new_destroyed_first
                    && new.target_freed
                    && new.returned.contains(&100)
                    && old.target_freed
                    && old.returned.contains(&100)
                    && id == 100
                    && a == 1
                    && b == 0
                    && !std::mem::replace(&mut self.retired_replacement_checked, true)
            }
            _ => return Ok(false),
        };
        if valid {
            Ok(true)
        } else {
            Err(format!("invalid handoff evidence {kind},{id},{a},{b}"))
        }
    }

    fn complete(&self, binding: Binding, domains: &[Evidence; 2]) -> bool {
        let [old, new] = domains;
        self.handoff.map(|h| h.address) == new.carrier[0]
            && self.stale_receipt_checked()
            && self.open_control.is_none()
            && self.new_sample.is_none()
            && self.new_checks == 3
            && self
                .first_target_destroyed
                .is_some_and(|first| self.retired_replacement_checked == (first == Domain::Second))
            && match binding {
                Binding::RustChannel => {
                    old.sdk_progress == usize::from(self.notified_before_environment_free)
                        && new.sdk_progress == 1
                }
                Binding::CDirect => old.sdk_progress == 0 && new.sdk_progress == 0,
                Binding::CJoined => false,
            }
    }
}

/// Replay the ordered interleaving of actual C or Rust events. Physical
/// receipt and target identities are bound before projecting to the shared
/// target model. The global trace retains the cross-Domain order of events.
///
/// # Errors
/// Rejects address aliases, foreign routing, incorrect reference counts,
/// or any incomplete or invalid shared-target projection.
pub fn replay_domains(
    binding: Binding,
    lines: &[String],
    limit: usize,
) -> Result<DomainReplayReport, String> {
    replay(binding, lines, limit, None)
}

/// Replay two notification Domains linked by a single physical storage handoff.
///
/// # Errors
/// Rejects invalid local traces, early reuse, stale receipt or producer
/// interference, and missing or inconsistent cross-Domain observations.
pub fn replay_reused_domains(
    binding: Binding,
    lines: &[String],
    limit: usize,
) -> Result<DomainReuseReplayReport, String> {
    let mut reuse = ReuseEvidence::default();
    let domains = replay(binding, lines, limit, Some(&mut reuse))?;
    Ok(DomainReuseReplayReport {
        domains,
        first_target_destroyed: reuse
            .first_target_destroyed
            .ok_or("missing target destruction")?,
        origin_environment_released_before_handoff: reuse
            .handoff
            .ok_or("missing handoff observation")?
            .environment_released,
        old_notified_while_replacement_unpublished: reuse.notified_while_unpublished,
        old_returned_while_replacement_unpublished: reuse.returned_while_unpublished,
    })
}

struct DomainEvent<'a> {
    domain: usize,
    kind: &'a str,
    id: u64,
    a: i64,
    b: i64,
}

impl<'a> DomainEvent<'a> {
    fn parse(line: &'a str) -> Result<Self, &'static str> {
        let mut fields = line.split(',');
        let domain = fields
            .next()
            .ok_or("missing Domain")?
            .parse()
            .map_err(|_| "unknown Domain")?;
        let kind = fields.next().ok_or("missing event kind")?;
        let id = fields
            .next()
            .ok_or("missing object id")?
            .parse()
            .map_err(|_| "invalid object id")?;
        let a = fields
            .next()
            .ok_or("missing argument")?
            .parse()
            .map_err(|_| "invalid argument")?;
        let b = fields
            .next()
            .ok_or("missing extra argument")?
            .parse()
            .map_err(|_| "invalid extra argument")?;
        if fields.next().is_some() {
            return Err("too many event fields");
        }
        Ok(Self {
            domain,
            kind,
            id,
            a,
            b,
        })
    }
}

fn check_addresses(domains: &[Evidence; 2], handoff: Option<StorageHandoff>) -> Result<(), String> {
    let mut physical = HashSet::new();
    for (index, evidence) in domains.iter().enumerate() {
        let skip_reused_address = usize::from(index == 1 && handoff.is_some());
        for object in evidence
            .carrier
            .into_iter()
            .take(evidence.members())
            .skip(skip_reused_address)
            .chain([evidence.context, evidence.target])
        {
            if !physical.insert(object.ok_or("missing physical identity")?) {
                return Err("physical identity shared by distinct live Domain objects".into());
            }
        }
        // The trace has already proved this allocation ended before any
        // replacement object was created. A malloc may reuse its address too.
        if index == 0
            && handoff.is_some_and(|h| h.environment_released)
            && let Some(context) = evidence.context
        {
            physical.remove(&context);
        }
    }
    Ok(())
}

fn replay(
    binding: Binding,
    lines: &[String],
    limit: usize,
    mut reuse: Option<&mut ReuseEvidence>,
) -> Result<DomainReplayReport, String> {
    let mut domains = [Evidence::default(), Evidence::default()];
    let mut concurrent_notifications = false;
    let mut free_during_foreign_notification = false;
    let mut notify_after_environment_free = false;
    let mut settlement_order = Vec::new();
    for (offset, line) in lines.iter().enumerate() {
        let DomainEvent {
            domain,
            kind,
            id,
            a,
            b,
        } = DomainEvent::parse(line).map_err(|e| format!("row {offset}: {e}"))?;
        let domain = *Domain::ALL
            .get(domain)
            .ok_or_else(|| format!("unknown Domain on row {offset}"))?;
        let (kind, a) = if let Some(handoff) = &mut reuse {
            if handoff
                .observe(&domains, domain, kind, id, a, b)
                .map_err(|e| format!("row {offset}: {e}"))?
            {
                continue;
            }
            if kind == "carrier_retire" {
                ("carrier_free", 0)
            } else {
                (kind, a)
            }
        } else {
            (kind, a)
        };
        let foreign_notifying = domains[domain.other().index()].notifying;
        let evidence = &mut domains[domain.index()];
        evidence
            .observe(kind, id, a, b)
            .map_err(|e| format!("row {offset}: {e}"))?;
        match kind {
            "notify_enter" => {
                concurrent_notifications |= foreign_notifying;
                notify_after_environment_free |= evidence.context_freed;
            }
            "target_identity_free" => free_during_foreign_notification |= foreign_notifying,
            "owner_settle" => settlement_order.push((
                domain,
                *evidence.receipts.get(&id).ok_or("unbound settlement")?,
            )),
            _ => {}
        }
    }
    if !domains.iter().all(|evidence| evidence.complete(binding)) {
        return Err("incomplete Domain observations".into());
    }
    if reuse
        .as_ref()
        .is_some_and(|evidence| !evidence.complete(binding, &domains))
    {
        return Err("incomplete cross-Domain handoff observations".into());
    }
    check_addresses(&domains, reuse.and_then(|evidence| evidence.handoff))?;
    Ok(DomainReplayReport {
        local: [
            replay_shared(
                binding,
                SharedTopology::Coalesced,
                &[7183, 0],
                &domains[0].rows,
                limit,
            )?,
            replay_shared(
                binding,
                SharedTopology::Coalesced,
                &[7280, 0],
                &domains[1].rows,
                limit,
            )?,
        ],
        concurrent_notifications,
        free_during_foreign_notification,
        notify_after_environment_free,
        settlement_order,
    })
}
