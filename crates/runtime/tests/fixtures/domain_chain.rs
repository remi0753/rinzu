use super::{c_fixture_trace, capture, return_reference, traced_reactor, with_c_binary};
use rinzu_runtime::lifetime_trace::{DomainTrace, Probe};
use rinzu_runtime::native_completion::{
    CapturedNativeWake, CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime::reactor::{AsyncReactor, ReactorOperation};
use rinzu_runtime::scheduler::{OperationId, Resolution, ResolveResult, TaskId};
use rinzu_runtime::standalone::OperationMailbox;
use rinzu_runtime_model::native_lifetime::{
    Binding, chain_orders, consecutive_orders, replay_consecutive_domains, replay_domain_chain,
};
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
struct Case {
    recovery: [usize; 6],
    teardown: &'static [usize],
    seed: usize,
    merged: bool,
}

impl Case {
    fn members(self) -> usize {
        2 + usize::from(self.merged)
    }

    fn cancelled(self, domain: usize) -> bool {
        self.seed % 4 == domain
    }

    fn detached(self, domain: usize) -> bool {
        !(self.seed + domain).is_multiple_of(2)
    }

    fn return_order(self, domain: usize) -> [usize; 2] {
        let first = (self.seed / 2 + domain) % 2;
        [first, 1 - first]
    }
}

fn cases(merged: bool) -> Vec<Case> {
    let (orders, teardowns): (_, &[&[usize]]) = if merged {
        (consecutive_orders(), &[&[0, 1], &[1, 0]])
    } else {
        (
            chain_orders(),
            &[
                &[0, 1, 2],
                &[0, 2, 1],
                &[1, 0, 2],
                &[1, 2, 0],
                &[2, 0, 1],
                &[2, 1, 0],
            ],
        )
    };
    let mut cases = Vec::new();
    for recovery in orders {
        for &teardown in teardowns {
            cases.push(Case {
                recovery,
                teardown,
                seed: cases.len(),
                merged,
            });
        }
    }
    assert_eq!(cases.len(), if merged { 80 } else { 90 });
    cases
}

fn check(binding: Binding, case: Case, events: &[String]) {
    let replay = |rows: &[String]| {
        if case.merged {
            replay_consecutive_domains(binding, rows, 100_000)
        } else {
            replay_domain_chain(binding, rows, 100_000)
        }
    };
    let report =
        replay(events).unwrap_or_else(|err| panic!("{case:?} {err}\n{}", events.join("\n")));
    assert_eq!(report.settlement_order, case.recovery);
    assert_eq!(report.destruction_order, case.teardown);
    for d in 0..case.teardown.len() {
        let expected: Vec<_> = case
            .merged
            .then_some(2)
            .into_iter()
            .chain(case.return_order(d))
            .collect();
        assert_eq!(report.return_orders[d], expected);
    }
    if case.seed == 0 {
        let row = |prefix: &str| {
            events
                .iter()
                .position(|event| event.starts_with(prefix))
                .unwrap()
        };
        let column = |index: usize| events[index].split(',').nth(3).unwrap();
        let last = case.teardown.len() - 1;
        let last_id = |prefix| row(&format!("{last},{prefix}"));
        for (index, wrong) in [
            (
                last_id("chain_handoff,"),
                format!(
                    "{last},chain_handoff,100,{},0",
                    column(row("0,carrier_address,101,"))
                ),
            ),
            (
                row("0,producer_refs,100,"),
                format!(
                    "0,producer_refs,100,{},2",
                    column(last_id("batch_target,100,"))
                ),
            ),
            (
                row("1,runtime_refs,"),
                format!("1,runtime_refs,0,{},2", column(row("1,batch_target,100,"))),
            ),
        ] {
            let mut corrupt = events.to_vec();
            corrupt[index] = wrong;
            assert!(replay(&corrupt).is_err(), "accepted an invalid chain trace");
        }
        let mut missing = events.to_vec();
        missing.remove(last_id("chain_final_slot,"));
        assert!(replay(&missing).is_err());
        if case.merged {
            let mut corrupt = events.to_vec();
            corrupt[row("0,merge_wake,102,")] = "0,merge_wake,102,100,0".into();
            assert!(
                replay(&corrupt).is_err(),
                "accepted a merge into the previous batch"
            );
            let sample = row("0,producer_refs,102,");
            corrupt = events.to_vec();
            corrupt[sample] = format!("0,producer_refs,102,{},3", column(sample));
            assert!(
                replay(&corrupt).is_err(),
                "accepted loss of the previous batch reference"
            );
        }
    }
    println!("CHAIN TRACE {binding:?} {case:?} events={}", events.len());
}

struct ChainDomain {
    d: usize,
    probe: Arc<Probe>,
    reactor: AsyncReactor,
    mailbox: OperationMailbox,
    storage: [Option<Box<TransferNativeCompletion>>; 3],
    receipts: Vec<(OperationId, ReactorOperation)>,
    references: [Option<CapturedNativeWake>; 2],
}

fn value(d: usize, g: usize) -> i64 {
    7183 + i64::try_from(97 * d + g).unwrap()
}

impl ChainDomain {
    fn new(
        timeline: &Arc<DomainTrace>,
        d: usize,
        scalar: Box<TransferNativeCompletion>,
        merged: bool,
    ) -> Self {
        let probe = timeline.probe(d);
        let _scope = Probe::install(Arc::clone(&probe));
        if d > 0 {
            assert_eq!(scalar.completion.notification.load(Ordering::Acquire), 1);
            let address = std::ptr::from_ref(&scalar.completion) as usize;
            probe.record("chain_handoff", 100, i64::try_from(address).unwrap(), 0);
        }
        let mut storage = [
            Some(scalar),
            Some(Box::default()),
            merged.then(Box::default),
        ];
        for (g, carrier) in storage
            .iter_mut()
            .map_while(Option::as_deref_mut)
            .enumerate()
        {
            *carrier = if g == 2 {
                TransferNativeCompletion {
                    completion: NotifiedNativeCompletion::default(),
                    result: 0,
                }
            } else {
                TransferNativeCompletion {
                    result: value(d, g),
                    ..Default::default()
                }
            };
            let address = std::ptr::from_ref(&carrier.completion) as usize;
            let token = 100 + u64::try_from(g).unwrap();
            probe.observe_carrier(address, token);
            probe.record("generation", token, i64::try_from(g).unwrap(), 0);
            probe.record(
                "carrier_address",
                token,
                i64::try_from(address).unwrap(),
                i64::from(carrier.completion.base.version),
            );
        }
        Self {
            d,
            reactor: traced_reactor(&probe, d),
            probe,
            mailbox: OperationMailbox::new(Default::default(), Arc::new(|| {})),
            storage,
            receipts: Vec::new(),
            references: [None, None],
        }
    }

    fn carrier_address(&self, g: usize) -> usize {
        std::ptr::from_ref(&self.storage[g].as_ref().unwrap().completion) as usize
    }

    fn admit(&mut self, g: usize, cancel: bool) {
        let _scope = Probe::install(Arc::clone(&self.probe));
        // SAFETY: the SDK owns this allocation until the receipt has copied the
        // completion, the worker has returned, and admission has closed.
        let completion = unsafe { CompletionPointer::new(self.carrier_address(g)) };
        let ticket = self
            .mailbox
            .admit(TaskId(1), "chained receipt".into())
            .unwrap();
        let id = ticket.operation;
        let receipt = if g == 2 {
            self.reactor.submit_native_completion(ticket, completion)
        } else {
            self.reactor.submit_native_transfer(ticket, completion)
        }
        .unwrap();
        self.probe.wait_for("registered", id.0, 1);
        if cancel {
            assert_eq!(receipt.request_cancel(), ResolveResult::Won);
            assert!(self.reactor.try_completion().unwrap().is_none());
        }
        self.receipts.push((id, receipt));
    }

    fn publish_first(&mut self) {
        self.admit(0, false);
        self.references[0] = Some(capture(&self.probe, 100, self.carrier_address(0)));
        self.copy_packets(0);
    }

    fn publish_following(&mut self, case: Case) {
        let controlled = case.cancelled(self.d);
        self.admit(1, controlled && !case.merged);
        if case.merged {
            self.admit(2, controlled);
        }
        let leader = capture(&self.probe, 101, self.carrier_address(1));
        assert_eq!(
            leader.address(),
            self.references[0].as_ref().unwrap().address()
        );
        if case.merged {
            let duplicate = capture(&self.probe, 102, self.carrier_address(2));
            assert_eq!(duplicate.address(), leader.address());
            self.probe.record("merge_wake", 102, 101, 0);
            return_reference(&self.probe, 102, duplicate);
        }
        self.references[1] = Some(leader);
        self.copy_packets(1);
    }

    fn copy_packets(&mut self, first: usize) {
        self.reactor.trace_progress().unwrap();
        self.probe.wait_for("trace_progress_refs", 0, first + 1);
        let mut seen = [false; 3];
        for _ in &self.receipts[first..] {
            let packet = self
                .reactor
                .wait_completion(Duration::from_secs(30))
                .unwrap();
            let g = self
                .receipts
                .iter()
                .position(|(id, _)| *id == packet.operation)
                .unwrap();
            assert!(g >= first && !std::mem::replace(&mut seen[g], true));
        }
        self.mailbox.take_resolutions().unwrap();
        for (id, _) in &self.receipts[first..] {
            self.probe.wait_for("worker_wake_end", id.0, 1);
        }
    }

    fn settle(&mut self, g: usize, cancelled: bool) {
        let (id, receipt) = &self.receipts[g];
        let expected = if cancelled {
            Resolution::Cancelled
        } else {
            Resolution::Completed(Ok(()))
        };
        assert_eq!(self.mailbox.settle(*id).unwrap(), expected);
        assert_eq!(
            receipt.native_transfer_result(),
            (g != 2).then_some(value(self.d, g))
        );
        let carrier = &self.storage[g].as_ref().unwrap().completion;
        let generation = 100 + u64::try_from(g).unwrap();
        self.probe.record(
            "generation_status",
            generation,
            i64::from(carrier.base.cancelled.load(Ordering::Acquire)),
            i64::from(carrier.notification.load(Ordering::Acquire) == 1),
        );
        self.probe.record("close_admission", generation, 0, 0);
        let address = std::ptr::from_ref(carrier) as usize;
        self.probe.record(
            "carrier_retire",
            generation,
            i64::try_from(address).unwrap(),
            0,
        );
    }

    fn notify_both(&self) {
        for (g, reference) in self.references.iter().enumerate() {
            let _producer = Probe::producer(100 + u64::try_from(g).unwrap());
            reference.as_ref().unwrap().notify();
        }
    }

    fn finish(&mut self, case: Case) {
        let detached = case.detached(self.d);
        if !detached {
            self.notify_both();
            self.probe.wait_for("sdk_progress", 0, 2);
        }
        assert_eq!(
            self.receipts[0].1.native_transfer_result(),
            Some(value(self.d, 0))
        );
        self.receipts.clear();
        self.probe.wait("record_release", case.members());
        self.reactor.shutdown();
        self.probe.wait("env_free", 1);
        assert!(self.mailbox.resources().used.is_baseline());
        if detached {
            self.notify_both();
        }
        for g in case.return_order(self.d) {
            return_reference(
                &self.probe,
                100 + u64::try_from(g).unwrap(),
                self.references[g].take().unwrap(),
            );
        }
        self.probe.wait("target_free", 1);
    }
}

#[test]
fn rust_reuses_one_carrier_in_three_domains_with_multiple_retained_refs() {
    rust_chain(false);
}

#[test]
fn rust_merges_a_later_batch_without_consuming_an_earlier_reference() {
    rust_chain(true);
}

fn rust_chain(merged: bool) {
    for case in cases(merged) {
        let timeline = Arc::new(DomainTrace::default());
        let mut owners: [Option<ChainDomain>; 3] = [None, None, None];
        for receipt in case.recovery {
            let (d, g) = (receipt / case.members(), receipt % case.members());
            if g == 0 {
                let scalar = if d == 0 {
                    Box::default()
                } else {
                    owners[d - 1].as_mut().unwrap().storage[0].take().unwrap()
                };
                let mut owner = ChainDomain::new(&timeline, d, scalar, merged);
                owner.publish_first();
                owner.settle(0, false);
                owner.publish_following(case);
                owners[d] = Some(owner);
            } else {
                owners[d]
                    .as_mut()
                    .unwrap()
                    .settle(g, case.cancelled(d) && g + 1 == case.members());
            }
        }
        for &d in case.teardown {
            owners[d].as_mut().unwrap().finish(case);
        }
        let last = owners[case.teardown.len() - 1].as_ref().unwrap();
        let scalar = last.storage[0].as_ref().unwrap();
        let slot = scalar.completion.notification.load(Ordering::Acquire);
        let cancelled = scalar.completion.base.cancelled.load(Ordering::Acquire);
        assert_eq!((slot, cancelled, scalar.result), (1, 0, value(last.d, 0)));
        last.probe.record(
            "chain_final_slot",
            100,
            i64::try_from(slot).unwrap(),
            i64::from(cancelled),
        );
        check(Binding::RustChannel, case, &timeline.events());
    }
}

fn digits(order: &[usize]) -> String {
    order
        .iter()
        .map(|&d| char::from(b'0' + u8::try_from(d).unwrap()))
        .collect()
}

#[test]
fn c_reuses_one_carrier_in_three_domains_with_multiple_retained_refs() {
    c_chain("chain", false);
}

#[test]
fn c_merges_a_later_batch_without_consuming_an_earlier_reference() {
    c_chain("consecutive", true);
}

fn c_chain(suite: &str, merged: bool) {
    with_c_binary(suite, "domain_chain.c", |binary, sanitized| {
        for case in cases(merged) {
            let events = c_fixture_trace(
                binary,
                &[
                    digits(&case.recovery),
                    digits(case.teardown),
                    case.seed.to_string(),
                    usize::from(merged).to_string(),
                ],
            );
            println!("CHAIN TRACE C sanitized={sanitized}");
            check(Binding::CDirect, case, &events);
        }
    });
}
