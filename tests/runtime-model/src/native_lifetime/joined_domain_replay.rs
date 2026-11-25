use super::{Domain, DomainEvent, ReplayReport, address};
use crate::native_lifetime::replay_joined;
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinedDomainReplayReport {
    pub local: [ReplayReport; 2],
    pub settlement_order: Vec<usize>,
    pub destruction_order: Vec<Domain>,
    pub replacement_published_before_registration: bool,
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "The helper, context, SDK capture and retirement have independent observed boundaries"
)]
struct Observed {
    runtime: Option<i64>,
    context: Option<i64>,
    carrier: [Option<i64>; 2],
    target: Option<i64>,
    primary: Option<u64>,
    polling: Option<u64>,
    carrier_bound: bool,
    settled: bool,
    retired: bool,
    registered: bool,
    early: bool,
    exchange: Option<bool>,
    capture: bool,
    helper_active: bool,
    helper_identity: bool,
    context_seen: bool,
    helper_accessed: bool,
    helper_joined: bool,
    context_freed: bool,
    runtime_refs: bool,
    producer_refs: bool,
    target_freed: bool,
    rows: Vec<String>,
}

impl Observed {
    #[allow(
        clippy::too_many_lines,
        reason = "The evidence is checked at its actual ordered lifetime boundary"
    )]
    fn observe(&mut self, event: &DomainEvent<'_>) -> Result<(), String> {
        let DomainEvent { kind, id, a, b, .. } = *event;
        let valid = match kind {
            "domain_runtime" => {
                id == 0 && b == 0 && address(a) && self.runtime.replace(a).is_none()
            }
            "domain_context" => {
                id == 0 && b == 0 && address(a) && self.context.replace(a).is_none()
            }
            "joined_carrier" => {
                id == 0 && b == 3 && address(a) && self.carrier[0].replace(a).is_none()
            }
            "joined_auxiliary" => {
                id == 0 && b == 1 && address(a) && self.carrier[1].replace(a).is_none()
            }
            "joined_carrier_identity" => {
                self.primary == Some(id)
                    && self.carrier[0] == Some(a)
                    && b == 0
                    && !std::mem::replace(&mut self.carrier_bound, true)
            }
            "joined_auxiliary_identity" => {
                id != 0
                    && self.primary != Some(id)
                    && self.carrier[1] == Some(a)
                    && b == 0
                    && self.polling.replace(id).is_none()
            }
            "wake_bind" => {
                self.primary == Some(id)
                    && self.carrier_bound
                    && address(a)
                    && b == 0
                    && self.target.replace(a).is_none()
            }
            "joined_capture" => {
                id == 0
                    && b == 0
                    && self.target == Some(a)
                    && self.exchange == Some(true)
                    && !std::mem::replace(&mut self.capture, true)
            }
            "helper_identity" => {
                id == 0
                    && b == 0
                    && self.runtime == Some(a)
                    && !std::mem::replace(&mut self.helper_identity, true)
            }
            "context_identity" => {
                id == 0
                    && b == 0
                    && self.context == Some(a)
                    && self.helper_active
                    && !self.context_freed
                    && !std::mem::replace(&mut self.context_seen, true)
            }
            "joined_carrier_retire" => {
                if !self.settled || id != 0 || b != 0 || self.carrier[0] != Some(a) || self.retired
                {
                    return Err("carrier retirement lacks the settled carrier identity".into());
                }
                self.retired = true;
                self.rows.push("carrier_free,0,0,0".into());
                return Ok(());
            }
            "runtime_refs" => {
                id == 0
                    && b == 1 + i64::from(self.capture)
                    && self.target == Some(a)
                    && !std::mem::replace(&mut self.runtime_refs, true)
            }
            "environment_identity_free" => {
                id == 0
                    && b == 0
                    && self.context == Some(a)
                    && self.helper_joined
                    && !std::mem::replace(&mut self.context_freed, true)
            }
            "joined_producer_refs" => {
                id == 0
                    && b == 1
                    && self.target == Some(a)
                    && self.capture
                    && self.context_freed
                    && !std::mem::replace(&mut self.producer_refs, true)
            }
            "target_identity_free" => {
                id == 0
                    && b == 0
                    && self.target == Some(a)
                    && self.helper_joined
                    && self.runtime_refs
                    && (!self.capture || self.producer_refs)
                    && !std::mem::replace(&mut self.target_freed, true)
            }
            _ => {
                match kind {
                    "admit" => {
                        if self.primary.replace(id).is_some() {
                            return Err("duplicate primary receipt".into());
                        }
                    }
                    "registered" => self.registered = true,
                    "sdk_exchange" => {
                        if self.exchange.replace(a == 1).is_some() {
                            return Err("duplicate final SDK exchange".into());
                        }
                        self.early = !self.registered;
                    }
                    "owner_settle" if self.primary == Some(id) => self.settled = true,
                    "helper_wake_begin" => {
                        if !self.helper_identity || self.helper_active {
                            return Err("unbound helper wake".into());
                        }
                        self.helper_active = true;
                    }
                    "helper_context" => {
                        if !self.helper_active
                            || !std::mem::take(&mut self.context_seen)
                            || std::mem::replace(&mut self.helper_accessed, true)
                        {
                            return Err("helper wake lacks actual context identity".into());
                        }
                    }
                    "helper_wake_end" => self.helper_active = false,
                    "helper_joined" if !self.helper_active => self.helper_joined = true,
                    "env_free" if !self.context_freed => {
                        return Err("missing actual context free".into());
                    }
                    "target_free" if !self.target_freed => {
                        return Err("missing actual target free".into());
                    }
                    "producer_release" if !self.producer_refs => {
                        return Err("missing SDK reference sample".into());
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
            Err(format!("invalid joined observation {kind},{id},{a},{b}"))
        }
    }

    fn complete(&self) -> bool {
        self.carrier_bound
            && self.polling.is_some()
            && self.helper_accessed
            && !self.context_seen
            && self.helper_joined
            && self.context_freed
            && self.target_freed
            && self.exchange == Some(self.capture)
            && self.capture == self.producer_refs
    }
}

/// Replay the interleaving of two real C helper lifetimes connected by
/// the address of a retired v3 carrier. The replacement may publish
/// before its own registration, while the origin retains its old target.
///
/// # Errors
/// Rejects early handoff, foreign identity, invalid local traces and
/// incomplete helper, target, or carrier observations.
pub fn replay_joined_handoff(
    lines: &[String],
    limit: usize,
) -> Result<JoinedDomainReplayReport, String> {
    let mut domains = [Observed::default(), Observed::default()];
    let mut handed_off = false;
    let mut final_slot = false;
    let mut settlement_order = Vec::new();
    let mut destruction_order = Vec::new();
    for (offset, row) in lines.iter().enumerate() {
        let event = DomainEvent::parse(row).map_err(|error| format!("row {offset}: {error}"))?;
        let d = event.domain;
        let domain = *Domain::ALL.get(d).ok_or("unknown joined Domain")?;
        if event.kind == "joined_handoff" {
            let old = &domains[0];
            if d != 1
                || handed_off
                || !old.retired
                || !old.helper_active
                || !old.capture
                || old.producer_refs
                || old.carrier[0] != Some(event.a)
                || event.id != 0
                || event.b != 0
            {
                return Err("handoff lacks a retired carrier, a retained SDK reference and an active helper".into());
            }
            handed_off = true;
            continue;
        }
        if d == 1 && !handed_off {
            return Err("replacement acted before handoff".into());
        }
        match event.kind {
            "joined_carrier" if d == 1 && domains[0].carrier[0] != Some(event.a) => {
                return Err("replacement did not reinitialize the retired carrier address".into());
            }
            "owner_settle" => settlement_order.push(d * 2),
            "aux_settle" => settlement_order.push(d * 2 + 1),
            "target_identity_free" => destruction_order.push(domain),
            "joined_final_slot" => {
                if d != 1
                    || final_slot
                    || event.id != 0
                    || event.a != 1
                    || event.b != 0
                    || !domains.iter().all(Observed::complete)
                {
                    return Err("invalid post-teardown carrier state".into());
                }
                final_slot = true;
                continue;
            }
            _ => {}
        }
        domains[d]
            .observe(&event)
            .map_err(|error| format!("row {offset}: {error}"))?;
    }
    if !handed_off || !final_slot || !domains.iter().all(Observed::complete) {
        return Err("incomplete joined handoff trace".into());
    }
    let mut live = HashSet::new();
    for (d, domain) in domains.iter().enumerate() {
        for identity in domain.carrier.into_iter().skip(d).chain([
            domain.runtime,
            domain.context,
            domain.target,
        ]) {
            if !live.insert(identity.ok_or("missing joined object address")?) {
                return Err("distinct live joined objects shared an address".into());
            }
        }
    }
    Ok(JoinedDomainReplayReport {
        local: [
            replay_joined(1, 7183, &domains[0].rows, limit)?,
            replay_joined(1, 7280, &domains[1].rows, limit)?,
        ],
        settlement_order,
        destruction_order,
        replacement_published_before_registration: domains[1].early,
    })
}
