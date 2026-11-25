use rinzu_runtime_model::native_lifetime::{
    Binding, Domain, DomainReuse, DomainReuseAction as A, DomainReuseFault, FailureKind, Protocol,
    SharedAction as S, StaleControl, reuse_orders,
};

fn step(model: &mut DomainReuse, action: A) {
    assert!(model.actions().contains(&action), "{action:?}");
    let before = model.clone();
    model.apply(action);
    before.check_step(action, model).unwrap();
    model.check().unwrap();
}

fn local(model: &mut DomainReuse, domain: Domain, actions: impl IntoIterator<Item = S>) {
    for action in actions {
        step(model, A::Local(domain, action));
    }
}

fn register(model: &mut DomainReuse, domain: Domain) {
    for g in 0..2 {
        local(
            model,
            domain,
            [
                S::Admit(g),
                S::Begin(g),
                S::Retain(g),
                S::Cas(g),
                S::CasObserved(g),
                S::Sample(g),
                S::Return(g),
            ],
        );
    }
    local(model, domain, [S::ControlBegin(1), S::ControlEnd(1)]);
}

fn publish(model: &mut DomainReuse, domain: Domain) {
    for g in 0..2 {
        local(
            model,
            domain,
            [
                S::PublishBegin(g),
                S::Prefix(g),
                S::Exchange(g),
                S::ExchangeObserved(g),
            ],
        );
    }
    local(
        model,
        domain,
        [
            S::MergeWake,
            S::ReleaseBegin(1),
            S::Decrement(1),
            S::ReleaseEnd(1),
            S::PublishEnd(1),
        ],
    );
}

fn begin_notification(model: &mut DomainReuse, domain: Domain, binding: Binding, attached: bool) {
    local(model, domain, [S::NotifyBegin(0)]);
    if binding == Binding::CDirect && attached {
        local(model, domain, [S::NotifyReady(0)]);
    }
}

fn notify(model: &mut DomainReuse, domain: Domain, binding: Binding, attached: bool) {
    begin_notification(model, domain, binding, attached);
    local(model, domain, [S::NotifyEnd(0)]);
}

fn copy(model: &mut DomainReuse, domain: Domain, binding: Binding) {
    for g in 0..2 {
        local(model, domain, [S::Copy(g), S::Physical(g)]);
        if binding == Binding::RustChannel {
            local(model, domain, [S::WakeBegin(g), S::WakeEnd(g)]);
        }
    }
}

fn collect(model: &mut DomainReuse, domain: Domain, g: usize) {
    local(model, domain, [S::Settle(g), S::Close(g), S::Retire(g)]);
}

fn free_environment(model: &mut DomainReuse, domain: Domain, binding: Binding) {
    local(
        model,
        domain,
        [S::ReleaseRecord(0), S::ReleaseRecord(1), S::Shutdown],
    );
    if binding == Binding::RustChannel {
        local(model, domain, [S::Quiescent, S::Join]);
    }
    assert_eq!(model.owner(domain).references(), 2);
    local(
        model,
        domain,
        [S::Detach, S::RuntimeRelease, S::EnvironmentFree],
    );
    assert_eq!(model.owner(domain).references(), 1);
}

fn return_leader(model: &mut DomainReuse, domain: Domain) {
    local(
        model,
        domain,
        [
            S::ReleaseBegin(0),
            S::Decrement(0),
            S::TargetFree,
            S::ReleaseEnd(0),
            S::PublishEnd(0),
        ],
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OldNotify {
    Attached,
    Detached,
    DetachedWhileUnpublished,
    Overlap,
    DetachedDuringPeer,
    DetachedBeforeHandoff,
}

impl OldNotify {
    fn early_teardown(self) -> bool {
        matches!(
            self,
            Self::DetachedWhileUnpublished | Self::DetachedDuringPeer | Self::DetachedBeforeHandoff
        )
    }
}

fn handoff(model: &mut DomainReuse, binding: Binding, timing: OldNotify, replacement_first: bool) {
    use Domain::{First, Second};
    step(model, A::Handoff);
    register(model, Second);
    step(model, A::ReadOldReceipt);
    for control in [StaleControl::Cancel, StaleControl::Timeout] {
        step(model, A::ControlOldReceipt(control));
    }
    if timing == OldNotify::Attached {
        notify(model, First, binding, true);
    }
    if timing == OldNotify::DetachedWhileUnpublished {
        return;
    }
    publish(model, Second);
    begin_notification(model, Second, binding, true);
    if timing == OldNotify::DetachedDuringPeer {
        return;
    }
    if timing == OldNotify::Overlap {
        begin_notification(model, First, binding, true);
        let first = if replacement_first { Second } else { First };
        for domain in [first, first.other()] {
            local(model, domain, [S::NotifyEnd(0)]);
        }
    } else {
        local(model, Second, [S::NotifyEnd(0)]);
    }
    copy(model, Second, binding);
}

fn close_origin_while_replacement_waits(
    model: &mut DomainReuse,
    binding: Binding,
    timing: OldNotify,
) {
    use Domain::{First, Second};
    free_environment(model, First, binding);
    if timing == OldNotify::DetachedBeforeHandoff {
        step(model, A::Handoff);
        register(model, Second);
    }
    notify(model, First, binding, false);
    let replacement = model.owner(Second).clone();
    return_leader(model, First);
    assert_eq!(model.owner(Second), &replacement);
    if timing != OldNotify::DetachedDuringPeer {
        publish(model, Second);
        begin_notification(model, Second, binding, true);
    }
    local(model, Second, [S::NotifyEnd(0)]);
    copy(model, Second, binding);
}

#[test]
fn exhausts_both_factors_of_the_dependent_reuse_model() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let [old, new] = DomainReuse::verify(binding, 500_000).unwrap();
        assert_eq!(old.allowed_stalls + new.allowed_stalls, 0);
        for witness in [
            "old-sdk-reference-survives-cross-domain-handoff",
            "old-sdk-reference-outlives-origin-after-handoff",
            "environment-released-before-cross-domain-handoff",
            "old-receipt-reads-cache-and-rejects-both-controls",
            "leader-settled-first",
            "covered-receipt-settled-first",
        ] {
            assert!(old.witnesses.contains(witness), "{witness}");
        }
        println!(
            "REUSE MODEL {binding:?} old {} states, {} edges, {} terminals; new {} states, {} edges, {} terminals",
            old.states, old.transitions, old.terminals, new.states, new.transitions, new.terminals
        );
        assert_eq!(
            DomainReuse::verify(binding, 1).unwrap_err().kind,
            FailureKind::StateLimit
        );
    }
}

#[test]
fn runs_every_legal_recovery_order_with_delayed_and_overlapping_notifications() {
    use Domain::{First, Second};
    let orders = reuse_orders();
    assert_eq!(orders.len(), 8);
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let mut count = 0;
        for order in &orders {
            for timing in [
                OldNotify::Attached,
                OldNotify::Detached,
                OldNotify::DetachedWhileUnpublished,
                OldNotify::Overlap,
                OldNotify::DetachedDuringPeer,
                OldNotify::DetachedBeforeHandoff,
            ] {
                let early = timing.early_teardown();
                if early && !order[..2].contains(&1) {
                    continue;
                }
                let mut model = DomainReuse::new(binding);
                register(&mut model, First);
                publish(&mut model, First);
                copy(&mut model, First, binding);
                for (index, &receipt) in order.iter().enumerate() {
                    let domain = Domain::ALL[receipt / 2];
                    collect(&mut model, domain, receipt % 2);
                    if receipt == 0 && timing != OldNotify::DetachedBeforeHandoff {
                        handoff(&mut model, binding, timing, order[1] == 1);
                    }
                    if early && index == 1 {
                        close_origin_while_replacement_waits(&mut model, binding, timing);
                    }
                }
                if !early {
                    free_environment(&mut model, First, binding);
                    if matches!(timing, OldNotify::Detached) {
                        notify(&mut model, First, binding, false);
                    }
                    return_leader(&mut model, First);
                }
                free_environment(&mut model, Second, binding);
                return_leader(&mut model, Second);
                assert!(model.terminal(), "{binding:?} {order:?} {timing:?}");
                count += 1;
            }
        }
        assert_eq!(count, 36);
        println!("REUSE MODEL {binding:?} {count} composed paths");
    }
}

#[test]
fn replacement_can_destroy_its_target_before_the_origin_notifies() {
    use Domain::{First, Second};
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let mut count = 0;
        for order in reuse_orders() {
            let mut product = DomainReuse::new(binding);
            register(&mut product, First);
            publish(&mut product, First);
            copy(&mut product, First, binding);
            for receipt in order {
                collect(&mut product, Domain::ALL[receipt / 2], receipt % 2);
                if receipt == 0 {
                    handoff(&mut product, binding, OldNotify::Detached, false);
                }
            }
            free_environment(&mut product, Second, binding);
            return_leader(&mut product, Second);
            assert!(product.owner(Second).terminal());
            assert_eq!(product.owner(First).references(), 2);
            for attached in [true, false] {
                let mut path = product.clone();
                if attached {
                    notify(&mut path, First, binding, true);
                }
                free_environment(&mut path, First, binding);
                if !attached {
                    notify(&mut path, First, binding, false);
                }
                return_leader(&mut path, First);
                assert_eq!(product.owner(Second), path.owner(Second));
                assert!(path.terminal());
                count += 1;
            }
        }
        assert_eq!(count, 16);
        println!("REUSE MODEL {binding:?} {count} paths with replacement target destroyed first");
    }
}

#[test]
fn rejects_early_handoff_foreign_notification_return_and_stale_control() {
    use Domain::{First, Second};
    use DomainReuseFault as F;
    let binding = Binding::CDirect;
    let mut model = DomainReuse::new(binding);
    register(&mut model, First);
    publish(&mut model, First);
    copy(&mut model, First, binding);
    local(&mut model, First, [S::Settle(0), S::Close(0)]);
    reject(&model, F::ReuseBeforeRetirement, A::Handoff, "NL-I4");
    local(&mut model, First, [S::Retire(0)]);
    step(&mut model, A::Handoff);
    register(&mut model, Second);
    let mut bad_cache = model.clone();
    bad_cache.inject(F::ReadReplacement);
    bad_cache.apply(A::ReadOldReceipt);
    assert_eq!(bad_cache.check().unwrap_err().invariant, "NL-I8");
    reject(
        &model,
        F::SignalReplacement,
        A::ControlOldReceipt(StaleControl::Timeout),
        "NL-I8",
    );
    let mut early_return = model.clone();
    notify(&mut early_return, First, binding, true);
    local(&mut early_return, First, [S::ReleaseBegin(0)]);
    reject(
        &early_return,
        F::ReturnReplacementReference,
        A::Local(First, S::Decrement(0)),
        "NL-I7",
    );
    publish(&mut model, Second);
    reject(
        &model,
        F::NotifyReplacement,
        A::Local(First, S::NotifyBegin(0)),
        "NL-I5",
    );
    notify(&mut model, First, binding, true);
    local(&mut model, First, [S::ReleaseBegin(0)]);
    reject(
        &model,
        F::ReturnReplacementReference,
        A::Local(First, S::Decrement(0)),
        "NL-I7",
    );
}

fn reject(model: &DomainReuse, fault: DomainReuseFault, action: A, invariant: &str) {
    let mut before = model.clone();
    before.inject(fault);
    assert!(before.actions().contains(&action));
    let mut after = before.clone();
    after.apply(action);
    assert_eq!(
        before.check_step(action, &after).unwrap_err().invariant,
        invariant
    );
}
