use super::{RustDomain, c_trace, with_c_binary};
use rinzu_runtime::lifetime_trace::DomainTrace;
use rinzu_runtime::scheduler::ResolveResult;
use rinzu_runtime_model::native_lifetime::{Binding, Domain, replay_reused_domains, reuse_orders};
use std::sync::{Arc, atomic::Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Timing {
    Attached,
    Detached,
    Unpublished,
    Overlap,
    DetachedDuringPeer,
    DetachedBeforeHandoff,
    AttachedAfterPeer,
    DetachedAfterPeer,
}

impl Timing {
    fn early_teardown(self) -> bool {
        matches!(
            self,
            Self::Unpublished | Self::DetachedDuringPeer | Self::DetachedBeforeHandoff
        )
    }

    fn replacement_first(self) -> bool {
        matches!(self, Self::AttachedAfterPeer | Self::DetachedAfterPeer)
    }
}

#[derive(Clone, Copy, Debug)]
struct Case {
    order: [usize; 4],
    cancelled_domain: usize,
    timing: Timing,
}

fn cases() -> Vec<Case> {
    let mut result = Vec::new();
    for order in reuse_orders() {
        for timing in [
            Timing::Attached,
            Timing::Detached,
            Timing::Unpublished,
            Timing::Overlap,
            Timing::DetachedDuringPeer,
            Timing::DetachedBeforeHandoff,
            Timing::AttachedAfterPeer,
            Timing::DetachedAfterPeer,
        ] {
            if timing.early_teardown() && !order[..2].contains(&1) {
                continue;
            }
            result.push(Case {
                order,
                cancelled_domain: result.len() % 3,
                timing,
            });
        }
    }
    assert_eq!(result.len(), 52);
    result
}

fn check(binding: Binding, case: Case, events: &[String]) {
    let replay = |rows: &[String]| replay_reused_domains(binding, rows, 100_000);
    let report =
        replay(events).unwrap_or_else(|error| panic!("{case:?} {error}\n{}", events.join("\n")));
    assert_eq!(
        report.origin_environment_released_before_handoff,
        case.timing == Timing::DetachedBeforeHandoff,
    );
    assert_eq!(
        report.domains.notify_after_environment_free,
        !matches!(
            case.timing,
            Timing::Attached | Timing::Overlap | Timing::AttachedAfterPeer
        ),
    );
    assert_eq!(
        report.first_target_destroyed,
        if case.timing.replacement_first() {
            Domain::Second
        } else {
            Domain::First
        },
    );
    assert_eq!(
        report.old_notified_while_replacement_unpublished,
        matches!(
            case.timing,
            Timing::Attached | Timing::Unpublished | Timing::DetachedBeforeHandoff
        ),
    );
    assert_eq!(
        report.old_returned_while_replacement_unpublished,
        matches!(
            case.timing,
            Timing::Unpublished | Timing::DetachedBeforeHandoff
        ),
    );
    assert_eq!(
        report.domains.concurrent_notifications,
        matches!(case.timing, Timing::Overlap | Timing::DetachedDuringPeer),
    );
    assert_eq!(
        report.domains.free_during_foreign_notification,
        case.timing == Timing::DetachedDuringPeer,
    );
    let actual: Vec<_> = report
        .domains
        .settlement_order
        .iter()
        .map(|(d, g)| d.index() * 2 + g)
        .collect();
    assert_eq!(actual, case.order, "{case:?}");
    if case.order == [0, 1, 2, 3] {
        check_rejects_corruptions(binding, events);
    }
    println!("REUSE TRACE {binding:?} {case:?} events={}", events.len());
}

fn check_rejects_corruptions(binding: Binding, events: &[String]) {
    let line = |prefix: &str| {
        events
            .iter()
            .position(|row| row.starts_with(prefix))
            .unwrap()
    };
    let old_target = events[line("0,batch_target,100,")]
        .split(',')
        .nth(3)
        .unwrap();
    let reject = |bad: &[String]| {
        assert!(
            replay_reused_domains(binding, bad, 100_000).is_err(),
            "replay accepted invalid reused-carrier evidence"
        );
    };
    let mut early = events.to_vec();
    let handoff = early.remove(line("1,storage_handoff,"));
    early.insert(line("0,carrier_retire,100,"), handoff);
    reject(&early);
    for (prefix, replacement) in [
        (
            "1,carrier_address,100,",
            "1,carrier_address,100,32,3".to_owned(),
        ),
        (
            "1,replacement_check,100,",
            format!("1,replacement_check,100,{old_target},0"),
        ),
        (
            "1,replacement_check,100,",
            "1,replacement_check,100,1,1".to_owned(),
        ),
        (
            "0,producer_refs,100,",
            "0,producer_refs,100,32,1".to_owned(),
        ),
    ] {
        let mut changed = events.to_vec();
        changed[line(prefix)] = replacement;
        reject(&changed);
    }
    if let Some(row) = events
        .iter()
        .position(|event| event.starts_with("0,old_receipt,"))
    {
        let mut wrong_cache = events.to_vec();
        wrong_cache[row] = wrong_cache[row].replace(",7183,0", ",7280,0");
        reject(&wrong_cache);
        let row = events
            .iter()
            .position(|event| event.starts_with("0,control_begin,") && event.ends_with(",2,0"))
            .unwrap();
        let mut missing_timeout = events.to_vec();
        missing_timeout.drain(row..row + 2);
        reject(&missing_timeout);
    }
    if binding == Binding::RustChannel {
        let row = line("1,sdk_progress,");
        let mut wrong_receiver = events.to_vec();
        wrong_receiver[row] = format!("1,sdk_progress,0,{old_target},0");
        reject(&wrong_receiver);
        let mut missing_receiver = events.to_vec();
        missing_receiver.remove(row);
        reject(&missing_receiver);
    }
    if let Some(row) = events
        .iter()
        .position(|event| event.starts_with("1,replacement_retired_check,"))
    {
        let mut missing_snapshot = events.to_vec();
        missing_snapshot.remove(row);
        reject(&missing_snapshot);
        let mut early_snapshot = events.to_vec();
        let event = early_snapshot.remove(row);
        early_snapshot.insert(line("0,producer_released,100,"), event);
        reject(&early_snapshot);
    }
}

fn replacement_check(replacement: &RustDomain, published: bool, sample: usize) {
    // Sample on the actual receiving reactor. This command is distinguished
    // from a nudge sent by an SDK target in the production trace.
    replacement.reactor.trace_progress().unwrap();
    replacement.probe.wait_for("trace_progress_refs", 0, sample);
    let carrier = &replacement.scalar.as_ref().unwrap().completion;
    let slot = carrier.notification.load(Ordering::Acquire);
    let cancelled = carrier.base.cancelled.load(Ordering::Acquire);
    assert_eq!(cancelled, 0);
    if published {
        assert_eq!(slot, 1);
    } else {
        assert!(slot > 1);
        assert!(replacement.reactor.try_completion().unwrap().is_none());
    }
    replacement.probe.record(
        "replacement_check",
        100,
        i64::try_from(slot).unwrap(),
        i64::from(cancelled),
    );
}

fn retired_replacement_check(replacement: &RustDomain) {
    let scalar = replacement.scalar.as_ref().unwrap();
    let slot = scalar.completion.notification.load(Ordering::Acquire);
    let cancelled = scalar.completion.base.cancelled.load(Ordering::Acquire);
    assert_eq!((slot, cancelled, scalar.result), (1, 0, replacement.value));
    replacement.probe.record(
        "replacement_retired_check",
        100,
        i64::try_from(slot).unwrap(),
        i64::from(cancelled),
    );
}

fn hold_replacement_notification(replacement: &mut RustDomain) {
    replacement.start_producer(false);
    replacement.probe.wait_for("notify_enter", 100, 1);
}

fn complete_replacement_notification(replacement: &mut RustDomain) {
    replacement.resume_notification();
    replacement.wait_for_packets();
}

fn overlap_notifications(old: &RustDomain, replacement: &mut RustDomain, replacement_first: bool) {
    hold_replacement_notification(replacement);
    old.probe.block_for("notify_enter", 100);
    old.probe.release_for("publish_end", 101);
    old.probe.wait_for("notify_enter", 100, 1);
    if replacement_first {
        replacement.resume_notification();
        old.resume_notification();
    } else {
        old.resume_notification();
        replacement.resume_notification();
    }
    replacement.wait_for_packets();
}

fn handoff(timeline: &Arc<DomainTrace>, old: &mut RustDomain, case: Case) -> RustDomain {
    let scalar = old.scalar.take().unwrap();
    let replacement =
        RustDomain::with_storage(timeline, 1, case.cancelled_domain == 1, Some(scalar));
    replacement_check(&replacement, false, 1);
    let (id, alias) = &old.receipts.as_ref().unwrap()[0];
    let cached = alias.native_transfer_result().unwrap();
    assert_eq!(cached, old.value);
    old.probe.record("old_receipt", id.0, cached, 0);
    assert_eq!(alias.request_cancel(), ResolveResult::Late);
    assert_eq!(alias.request_timeout(), ResolveResult::Late);
    if case.timing == Timing::Attached {
        old.probe.release_for("publish_end", 101);
        old.probe.wait_for("notify_leave", 100, 1);
        old.probe.wait_for("sdk_progress", 0, 1);
    }
    replacement_check(&replacement, false, 2);
    replacement
}

fn reuse_after_origin_dispose(
    timeline: &Arc<DomainTrace>,
    old: &mut RustDomain,
    case: Case,
) -> RustDomain {
    old.dispose();
    let scalar = old.scalar.take().unwrap();
    let mut replacement =
        RustDomain::with_storage(timeline, 1, case.cancelled_domain == 1, Some(scalar));
    replacement_check(&replacement, false, 1);
    old.probe.release_for("publish_end", 101);
    old.probe.wait_for("notify_leave", 100, 1);
    replacement_check(&replacement, false, 2);
    old.return_sdk_reference();
    replacement_check(&replacement, false, 3);
    hold_replacement_notification(&mut replacement);
    complete_replacement_notification(&mut replacement);
    replacement
}

#[test]
fn rust_reuses_a_carrier_while_its_old_sdk_reference_is_live() {
    for case in cases() {
        let timeline = Arc::new(DomainTrace::default());
        let mut old = RustDomain::new(&timeline, 0, case.cancelled_domain == 0);
        old.start_producer(true);
        old.probe.wait_for("publish_end", 101, 1);
        old.reactor.trace_progress().unwrap();
        old.wait_for_packets();
        let mut new: Option<RustDomain> = None;
        for (index, choice) in case.order.into_iter().enumerate() {
            if choice < 2 {
                old.settle(choice, true);
            } else {
                new.as_mut().unwrap().settle(choice - 2, true);
            }
            if choice == 0 && case.timing != Timing::DetachedBeforeHandoff {
                let mut replacement = handoff(&timeline, &mut old, case);
                match case.timing {
                    Timing::Attached
                    | Timing::Detached
                    | Timing::AttachedAfterPeer
                    | Timing::DetachedAfterPeer => {
                        hold_replacement_notification(&mut replacement);
                        complete_replacement_notification(&mut replacement);
                    }
                    Timing::Overlap => {
                        overlap_notifications(&old, &mut replacement, case.order[1] == 1);
                    }
                    Timing::DetachedDuringPeer => hold_replacement_notification(&mut replacement),
                    Timing::Unpublished => {}
                    Timing::DetachedBeforeHandoff => unreachable!(),
                }
                new = Some(replacement);
            }
            if index == 1 && case.timing.early_teardown() {
                if case.timing == Timing::DetachedBeforeHandoff {
                    new = Some(reuse_after_origin_dispose(&timeline, &mut old, case));
                    continue;
                }
                old.finish();
                let replacement = new.as_mut().unwrap();
                let published = case.timing != Timing::Unpublished;
                replacement_check(replacement, published, 3);
                if !published {
                    hold_replacement_notification(replacement);
                }
                complete_replacement_notification(replacement);
            }
        }
        let mut new = new.unwrap();
        if case.timing.replacement_first() {
            replacement_check(&new, true, 3);
            new.finish();
            if case.timing == Timing::AttachedAfterPeer {
                old.probe.release_for("publish_end", 101);
                old.probe.wait_for("sdk_progress", 0, 1);
            }
            old.finish();
            retired_replacement_check(&new);
        } else {
            if !case.timing.early_teardown() {
                old.finish();
                replacement_check(&new, true, 3);
            }
            new.finish();
        }
        check(Binding::RustChannel, case, &timeline.events());
    }
}

#[test]
fn c_reuses_a_carrier_while_its_old_sdk_reference_is_live() {
    with_c_binary("reuse", "domain_batches.c", |binary, sanitized| {
        for case in cases() {
            let schedule = match case.timing {
                Timing::Attached => 0,
                Timing::Detached => 1,
                Timing::Unpublished => 2,
                Timing::Overlap => 3,
                Timing::DetachedDuringPeer => 4,
                Timing::DetachedBeforeHandoff => 5,
                Timing::AttachedAfterPeer => 6,
                Timing::DetachedAfterPeer => 7,
            };
            let events = c_trace(binary, case.order, case.cancelled_domain, schedule, true);
            println!("REUSE TRACE C sanitized={sanitized}");
            check(Binding::CDirect, case, &events);
        }
    });
}
