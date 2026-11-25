use rinzu_runtime_model::native_lifetime::{
    Binding, Domain, DomainAction, DomainBatches, DomainFault, Protocol, SharedAction as A,
};

fn step(model: &mut DomainBatches, domain: Domain, action: A) {
    let action = DomainAction::Local(domain, action);
    assert!(model.actions().contains(&action), "{action:?}");
    let before = model.clone();
    model.apply(action);
    before.check_step(action, model).unwrap();
    model.check().unwrap();
}

fn prepare(binding: Binding) -> DomainBatches {
    let mut model = DomainBatches::new(binding);
    for domain in Domain::ALL {
        for g in 0..2 {
            for action in [
                A::Admit(g),
                A::Begin(g),
                A::Retain(g),
                A::Cas(g),
                A::CasObserved(g),
                A::Sample(g),
                A::Return(g),
            ] {
                step(&mut model, domain, action);
            }
        }
        step(&mut model, domain, A::ControlBegin(1));
        step(&mut model, domain, A::ControlEnd(1));
    }
    for g in 0..2 {
        for domain in Domain::ALL {
            for action in [
                A::PublishBegin(g),
                A::Prefix(g),
                A::Exchange(g),
                A::ExchangeObserved(g),
            ] {
                step(&mut model, domain, action);
            }
        }
    }
    model
}

fn merge(model: &mut DomainBatches) {
    for domain in Domain::ALL {
        assert_eq!(model.owner(domain).references(), 3);
        for action in [
            A::MergeWake,
            A::ReleaseBegin(1),
            A::Decrement(1),
            A::ReleaseEnd(1),
            A::PublishEnd(1),
        ] {
            step(model, domain, action);
        }
    }
}

fn begin_notification(model: &mut DomainBatches, binding: Binding, domain: Domain) {
    step(model, domain, A::NotifyBegin(0));
    if binding == Binding::CDirect {
        step(model, domain, A::NotifyReady(0));
    }
}

fn begin_notifications(model: &mut DomainBatches, binding: Binding) {
    merge(model);
    for domain in Domain::ALL {
        begin_notification(model, binding, domain);
    }
}

fn copy_receipts(model: &mut DomainBatches, binding: Binding, domain: Domain) {
    for g in 0..2 {
        for action in [A::Copy(g), A::Physical(g)] {
            step(model, domain, action);
        }
        if binding == Binding::RustChannel {
            step(model, domain, A::WakeBegin(g));
            step(model, domain, A::WakeEnd(g));
        }
    }
}

fn physical(model: &mut DomainBatches, binding: Binding, domain: Domain) {
    step(model, domain, A::NotifyEnd(0));
    copy_receipts(model, binding, domain);
}

fn collect(model: &mut DomainBatches, domain: Domain, g: usize) {
    for action in [A::Settle(g), A::Close(g), A::Retire(g), A::ReleaseRecord(g)] {
        step(model, domain, action);
    }
}

fn shutdown(model: &mut DomainBatches, binding: Binding, domain: Domain) {
    step(model, domain, A::Shutdown);
    if binding == Binding::RustChannel {
        step(model, domain, A::Quiescent);
        step(model, domain, A::Join);
    }
}

fn free_environment(model: &mut DomainBatches, binding: Binding, domain: Domain) {
    shutdown(model, binding, domain);
    assert_eq!(model.owner(domain).references(), 2);
    for action in [A::Detach, A::RuntimeRelease, A::EnvironmentFree] {
        step(model, domain, action);
    }
    assert_eq!(model.owner(domain).references(), 1);
}

fn return_leader(model: &mut DomainBatches, domain: Domain) {
    for action in [
        A::ReleaseBegin(0),
        A::Decrement(0),
        A::TargetFree,
        A::ReleaseEnd(0),
        A::PublishEnd(0),
    ] {
        step(model, domain, action);
    }
}

fn finish(model: &mut DomainBatches, binding: Binding, domain: Domain) {
    free_environment(model, binding, domain);
    return_leader(model, domain);
}

#[test]
fn exhausts_two_notification_domains_by_exact_factorization() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let report =
            DomainBatches::verify(binding, 500_000).unwrap_or_else(|failure| panic!("{failure}"));
        for local in &report.local {
            assert_eq!(local.allowed_stalls, 0);
            assert!(local.witnesses.contains("leader-settled-first"));
            assert!(local.witnesses.contains("covered-receipt-settled-first"));
            assert!(
                local
                    .witnesses
                    .contains("sdk-reference-outlives-shared-environment")
            );
        }
        println!(
            "DOMAIN MODEL {binding:?} local {} states, {} edges, {} terminals; product {} states, {} edges, {} terminals",
            report.local[0].states,
            report.local[0].transitions,
            report.local[0].terminals,
            report.states,
            report.transitions,
            report.terminals
        );
    }
}

#[test]
fn all_ready_collection_orders_and_either_teardown_survive_overlap() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let mut both = prepare(binding);
        begin_notifications(&mut both, binding);
        for first in Domain::ALL {
            let mut held = both.clone();
            physical(&mut held, binding, first);
            collect(&mut held, first, 1);
            collect(&mut held, first, 0);
            finish(&mut held, binding, first);
            assert!(held.owner(first).terminal());
            assert_eq!(held.owner(first.other()).references(), 2);
            physical(&mut held, binding, first.other());
            collect(&mut held, first.other(), 0);
            collect(&mut held, first.other(), 1);
            finish(&mut held, binding, first.other());
            assert!(held.terminal());
        }
        for domain in Domain::ALL {
            physical(&mut both, binding, domain);
        }
        let mut orders = 0;
        let ids = [0, 1, 2, 3];
        for a in ids {
            for b in ids.into_iter().filter(|&b| b != a) {
                for c in ids.into_iter().filter(|&c| c != a && c != b) {
                    let d = ids
                        .into_iter()
                        .find(|&d| d != a && d != b && d != c)
                        .unwrap();
                    let mut product = both.clone();
                    let mut remaining = [2; 2];
                    for receipt in [a, b, c, d] {
                        let domain = Domain::ALL[receipt / 2];
                        collect(&mut product, domain, receipt % 2);
                        remaining[domain.index()] -= 1;
                        if remaining[domain.index()] == 0 {
                            finish(&mut product, binding, domain);
                        }
                    }
                    assert!(product.terminal());
                    orders += 1;
                }
            }
        }
        assert_eq!(orders, 24);
        println!(
            "DOMAIN MODEL {binding:?} {orders} collection orders, 2 teardown orders with a foreign notification active"
        );
    }
}

#[test]
fn a_detached_batch_can_notify_while_the_peer_is_still_notifying() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        for first in Domain::ALL {
            let mut model = prepare(binding);
            merge(&mut model);
            let peer = first.other();
            begin_notification(&mut model, binding, peer);
            copy_receipts(&mut model, binding, first);
            collect(&mut model, first, 0);
            collect(&mut model, first, 1);
            free_environment(&mut model, binding, first);
            step(&mut model, first, A::NotifyBegin(0));
            step(&mut model, first, A::NotifyEnd(0));
            return_leader(&mut model, first);
            assert!(model.owner(first).terminal());
            assert_eq!(model.owner(peer).references(), 2);
            physical(&mut model, binding, peer);
            collect(&mut model, peer, 1);
            collect(&mut model, peer, 0);
            finish(&mut model, binding, peer);
            assert!(model.terminal());
        }
        println!("DOMAIN MODEL {binding:?} both detached notification orders survived");
    }
}

#[test]
fn rejects_foreign_reference_return_and_foreign_detachment() {
    let binding = Binding::CDirect;
    for domain in Domain::ALL {
        let mut model = prepare(binding);
        for action in [A::MergeWake, A::ReleaseBegin(1)] {
            step(&mut model, domain, action);
        }
        model.fault = DomainFault::ReturnForeignReference;
        let action = DomainAction::Local(domain, A::Decrement(1));
        assert!(model.actions().contains(&action));
        let mut after = model.clone();
        after.apply(action);
        assert_eq!(
            model.check_step(action, &after).unwrap_err().invariant,
            "NL-I7"
        );

        let mut model = prepare(binding);
        begin_notifications(&mut model, binding);
        physical(&mut model, binding, domain);
        collect(&mut model, domain, 0);
        collect(&mut model, domain, 1);
        shutdown(&mut model, binding, domain);
        model.fault = DomainFault::DetachForeignEnvironment;
        let action = DomainAction::Local(domain, A::Detach);
        assert!(model.actions().contains(&action));
        let mut after = model.clone();
        after.apply(action);
        assert_eq!(
            model.check_step(action, &after).unwrap_err().invariant,
            "NL-I5"
        );
    }
}
