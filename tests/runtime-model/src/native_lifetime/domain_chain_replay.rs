use super::{
    Binding, DomainEvent, Evidence, EvidenceProfile, ReplayReport, SharedTopology, member,
    replay_shared,
};
use std::collections::HashSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainChainReplayReport {
    pub local: Vec<ReplayReport>,
    pub settlement_order: Vec<usize>,
    pub destruction_order: Vec<usize>,
    pub return_orders: [Vec<usize>; 3],
}

#[derive(Default)]
struct ChainEvidence {
    latest: usize,
    scalar_retired: [bool; 3],
    scalar_settled: [bool; 3],
    sampled_both_references: [bool; 3],
    sdk_delivery_expected: [usize; 3],
    settlement_order: Vec<usize>,
    destruction_order: Vec<usize>,
    return_orders: [Vec<usize>; 3],
    final_slot_checked: bool,
}

impl ChainEvidence {
    fn observe(&mut self, domains: &[Evidence], event: &DomainEvent<'_>) -> Result<bool, String> {
        let DomainEvent {
            domain: d,
            kind,
            id,
            a,
            b,
        } = *event;
        if d > self.latest || d >= domains.len() {
            if kind == "chain_handoff"
                && d == self.latest + 1
                && let Some(previous) = domains.get(d - 1)
                && d < domains.len()
                && previous.carrier[0] == Some(a)
                && self.scalar_retired[d - 1]
                && previous.captures.len() == previous.members()
                && self.sampled_both_references[d - 1]
                && !previous.notified
                && previous.sdk_references() == 2
                && id == 100
                && b == 0
            {
                self.latest = d;
                return Ok(true);
            }
            return Err("next Domain lacks a valid carrier handoff".into());
        }
        let local = &domains[d];
        let members = local.members();
        match kind {
            "chain_handoff" => return Err("duplicate carrier handoff".into()),
            "carrier_address" if d > 0 && id == 100 && domains[0].carrier[0] != Some(a) => {
                return Err("new scalar carrier did not reuse the handed-off address".into());
            }
            "owner_settle" => {
                let generation = *local.receipts.get(&id).ok_or("unbound settlement")?;
                self.scalar_settled[d] |= generation == 0;
                self.settlement_order.push(d * members + generation);
            }
            "carrier_retire" => {
                let generation = member(id)?;
                if local.carrier[generation] != Some(a)
                    || (generation == 0 && !self.scalar_settled[d])
                    || b != 0
                {
                    return Err("retirement lacks the settled carrier identity".into());
                }
                self.scalar_retired[d] |= generation == 0;
            }
            "trace_progress_refs" | "chain_refs" => {
                if id != 0
                    || local.context_freed
                    || local.target != Some(a)
                    || usize::try_from(b).ok() != Some(local.sdk_references() + 1)
                {
                    return Err("published references do not match the retained target".into());
                }
                self.sampled_both_references[d] |= b == 3;
                return Ok(true);
            }
            "notify_enter" => {
                if d + 1 < domains.len() && self.latest == d {
                    return Err("sender notified before transferring its carrier".into());
                }
                self.sdk_delivery_expected[d] += usize::from(!local.context_freed);
            }
            "target_identity_free" => self.destruction_order.push(d),
            "producer_released" => self.return_orders[d].push(member(id)?),
            "chain_final_slot" => {
                if d + 1 != domains.len()
                    || id != 100
                    || a != 1
                    || b != 0
                    || self.final_slot_checked
                    || domains
                        .iter()
                        .any(|lane| !lane.target_freed || lane.returned.len() != members)
                {
                    return Err("missing or changed final carrier state".into());
                }
                self.final_slot_checked = true;
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    fn complete(&self, binding: Binding, domains: &[Evidence]) -> bool {
        self.latest + 1 == domains.len()
            && self.sampled_both_references[..domains.len()]
                .iter()
                .all(|&sampled| sampled)
            && self.final_slot_checked
            && domains.iter().enumerate().all(|(d, lane)| {
                lane.complete(binding)
                    && (binding != Binding::RustChannel
                        || lane.sdk_progress == self.sdk_delivery_expected[d])
            })
    }
}

/// Checks a single actual address reused by three Domains in order. Each
/// Domain retains two independent notifications and may destroy its target
/// before or after either peer.
///
/// # Errors
/// Rejects unsafe transfer, foreign routing, inconsistent reference counts,
/// incomplete observations, and failures of the local protocol replay.
pub fn replay_domain_chain(
    binding: Binding,
    lines: &[String],
    limit: usize,
) -> Result<DomainChainReplayReport, String> {
    replay(binding, lines, limit, EvidenceProfile::Separate, 3)
}

/// Checks two Domains with a single-publisher batch and a subsequent merged
/// batch in each. The earlier captured reference survives duplicate return
/// and handoff of its carrier's address to the second Domain.
///
/// # Errors
/// Rejects unsafe handoff, counts, notification routing or incomplete evidence.
pub fn replay_consecutive_domains(
    binding: Binding,
    lines: &[String],
    limit: usize,
) -> Result<DomainChainReplayReport, String> {
    replay(binding, lines, limit, EvidenceProfile::Consecutive, 2)
}

fn replay(
    binding: Binding,
    lines: &[String],
    limit: usize,
    profile: EvidenceProfile,
    domain_count: usize,
) -> Result<DomainChainReplayReport, String> {
    if binding == Binding::CJoined {
        return Err("joined C is outside this chain profile".into());
    }
    let mut domains: Vec<_> = (0..domain_count)
        .map(|_| Evidence {
            profile,
            ..Evidence::default()
        })
        .collect();
    let mut chain = ChainEvidence::default();
    for (index, line) in lines.iter().enumerate() {
        let event = DomainEvent::parse(line).map_err(|e| format!("row {index}: {e}"))?;
        if !chain
            .observe(&domains, &event)
            .map_err(|e| format!("row {index}: {e}"))?
        {
            let DomainEvent {
                domain,
                kind,
                id,
                a,
                b,
            } = event;
            let (kind, a) = if kind == "carrier_retire" {
                ("carrier_free", 0)
            } else {
                (kind, a)
            };
            domains[domain]
                .observe(kind, id, a, b)
                .map_err(|e| format!("row {index}: {e}"))?;
        }
    }
    if !chain.complete(binding, &domains) {
        return Err("incomplete Domain chain".into());
    }
    let mut live_identities = HashSet::new();
    for (d, lane) in domains.iter().enumerate() {
        for identity in lane
            .carrier
            .into_iter()
            .take(lane.members())
            .skip(usize::from(d != 0))
            .chain([lane.context, lane.target])
        {
            if !live_identities.insert(identity.ok_or("missing chain object address")?) {
                return Err("simultaneously live chain objects shared an address".into());
            }
        }
    }
    let local = domains
        .iter()
        .enumerate()
        .map(|(d, lane)| {
            let scalar = 7183 + 97 * i64::try_from(d).map_err(|_| "invalid Domain number")?;
            let (topology, payload) = if profile == EvidenceProfile::Consecutive {
                (SharedTopology::Consecutive, vec![scalar, scalar + 1, 0])
            } else {
                (SharedTopology::Carriers, vec![scalar, scalar + 1])
            };
            replay_shared(binding, topology, &payload, &lane.rows, limit)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DomainChainReplayReport {
        local,
        settlement_order: chain.settlement_order,
        destruction_order: chain.destruction_order,
        return_orders: chain.return_orders,
    })
}
