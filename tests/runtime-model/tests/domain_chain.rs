use rinzu_runtime_model::native_lifetime::{
    Binding, ChainAction as A, ChainFault, DomainChain, Protocol, SharedAction as S, SharedFault,
    chain_orders, consecutive_orders,
};

type Chain = DomainChain<3>;

fn step<const N: usize>(chain: &mut DomainChain<N>, action: A) {
    assert!(chain.actions().contains(&action), "{action:?}");
    let before = chain.clone();
    chain.apply(action);
    before.check_step(action, chain).unwrap();
    chain.check().unwrap();
}

fn local<const N: usize>(
    chain: &mut DomainChain<N>,
    d: usize,
    actions: impl IntoIterator<Item = S>,
) {
    for action in actions {
        step(chain, A::Local(d, action));
    }
}

fn register<const N: usize>(chain: &mut DomainChain<N>, d: usize, g: usize) {
    local(
        chain,
        d,
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

fn copy<const N: usize>(chain: &mut DomainChain<N>, binding: Binding, d: usize, g: usize) {
    publish(chain, d, g);
    acquire(chain, binding, d, g);
}

fn publish<const N: usize>(chain: &mut DomainChain<N>, d: usize, g: usize) {
    local(
        chain,
        d,
        [
            S::PublishBegin(g),
            S::Prefix(g),
            S::Exchange(g),
            S::ExchangeObserved(g),
        ],
    );
}

fn acquire<const N: usize>(chain: &mut DomainChain<N>, binding: Binding, d: usize, g: usize) {
    local(chain, d, [S::Copy(g), S::Physical(g)]);
    if binding == Binding::RustChannel {
        local(chain, d, [S::WakeBegin(g), S::WakeEnd(g)]);
    }
}

fn collect<const N: usize>(chain: &mut DomainChain<N>, d: usize, g: usize) {
    local(chain, d, [S::Settle(g), S::Close(g), S::Retire(g)]);
}

fn notify<const N: usize>(
    chain: &mut DomainChain<N>,
    binding: Binding,
    d: usize,
    g: usize,
    attached: bool,
) {
    local(chain, d, [S::NotifyBegin(g)]);
    if binding == Binding::CDirect && attached {
        local(chain, d, [S::NotifyReady(g)]);
    }
    local(chain, d, [S::NotifyEnd(g)]);
}

fn finish<const N: usize>(
    chain: &mut DomainChain<N>,
    binding: Binding,
    d: usize,
    detached: bool,
    order: [usize; 2],
    members: usize,
) {
    if !detached {
        for g in order {
            notify(chain, binding, d, g, true);
        }
    }
    assert_eq!(chain.owner(d).references(), 3);
    local(chain, d, (0..members).map(S::ReleaseRecord));
    local(chain, d, [S::Shutdown]);
    if binding == Binding::RustChannel {
        local(chain, d, [S::Quiescent, S::Join]);
    }
    local(chain, d, [S::Detach, S::RuntimeRelease, S::EnvironmentFree]);
    assert_eq!(chain.owner(d).references(), 2);
    if detached {
        for g in order {
            notify(chain, binding, d, g, false);
        }
    }
    for (i, g) in order.into_iter().enumerate() {
        assert_eq!(chain.owner(d).references(), 2 - u8::try_from(i).unwrap());
        local(chain, d, [S::ReleaseBegin(g), S::Decrement(g)]);
        if i == 1 {
            local(chain, d, [S::TargetFree]);
        }
        local(chain, d, [S::ReleaseEnd(g), S::PublishEnd(g)]);
    }
}

#[test]
fn exhausts_the_two_factors_of_a_serial_domain_handoff() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let [sender, last] = Chain::verify(binding, 500_000).unwrap();
        assert_eq!(sender.allowed_stalls + last.allowed_stalls, 0);
        assert!(
            sender
                .witnesses
                .contains("two-sdk-references-survive-handoff")
        );
        assert!(sender.witnesses.contains("two-producers-outlive-runtime"));
        println!(
            "CHAIN MODEL {binding:?} sender {} states, {} edges, {} terminals; terminal factor {} states, {} edges, {} terminals",
            sender.states,
            sender.transitions,
            sender.terminals,
            last.states,
            last.transitions,
            last.terminals,
        );
    }
}

#[test]
fn explores_all_recovery_and_teardown_orders_across_three_domains() {
    let orders = chain_orders();
    assert_eq!(orders.len(), 15);
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let mut paths = 0usize;
        for order in &orders {
            let mut chain = Chain::new(binding);
            for &receipt in order {
                let (d, g) = (receipt / 2, receipt % 2);
                if g == 0 {
                    register(&mut chain, d, 0);
                    copy(&mut chain, binding, d, 0);
                    collect(&mut chain, d, 0);
                    register(&mut chain, d, 1);
                    local(&mut chain, d, [S::ControlBegin(1), S::ControlEnd(1)]);
                    copy(&mut chain, binding, d, 1);
                    if d < 2 {
                        step(&mut chain, A::Handoff(d));
                        assert_eq!(chain.owner(d).references(), 3);
                    }
                } else {
                    collect(&mut chain, d, g);
                }
            }
            for teardown in [
                [0, 1, 2],
                [0, 2, 1],
                [1, 0, 2],
                [1, 2, 0],
                [2, 0, 1],
                [2, 1, 0],
            ] {
                let mut path = chain.clone();
                for d in teardown {
                    let detached = !(paths + d).is_multiple_of(2);
                    let pair = if (paths / 2 + d).is_multiple_of(2) {
                        [0, 1]
                    } else {
                        [1, 0]
                    };
                    finish(&mut path, binding, d, detached, pair, 2);
                }
                assert!(path.terminal());
                paths += 1;
            }
        }
        assert_eq!(paths, 90);
        println!("CHAIN MODEL {binding:?} {paths} composed paths");
    }
}

fn prepare_consecutive(chain: &mut DomainChain<2>, binding: Binding, d: usize) {
    register(chain, d, 0);
    copy(chain, binding, d, 0);
    collect(chain, d, 0);
    register(chain, d, 1);
    register(chain, d, 2);
    local(chain, d, [S::ControlBegin(2), S::ControlEnd(2)]);
    publish(chain, d, 1);
    publish(chain, d, 2);
    assert_eq!(chain.owner(d).references(), 4);
}

#[test]
fn exhausts_factors_with_a_single_then_a_merged_batch() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let [sender, last] =
            DomainChain::<2>::verify_consecutive_batches(binding, 2_000_000).unwrap();
        assert_eq!(sender.allowed_stalls + last.allowed_stalls, 0);
        for witness in [
            "two-sdk-references-survive-handoff",
            "duplicate-released-while-batch-target-retained",
            "four-target-references",
        ] {
            assert!(sender.witnesses.contains(witness), "{witness}");
        }
        println!(
            "CONSECUTIVE MODEL {binding:?} sender {} states, {} edges, {} terminals; terminal factor {} states, {} edges, {} terminals",
            sender.states,
            sender.transitions,
            sender.terminals,
            last.states,
            last.transitions,
            last.terminals,
        );
    }
}

#[test]
fn a_later_merge_preserves_earlier_captures_across_all_recovery_orders() {
    let orders = consecutive_orders();
    assert_eq!(orders.len(), 40);
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let mut paths = 0usize;
        for order in &orders {
            let mut chain = DomainChain::with_consecutive_batches(binding);
            for &receipt in order {
                let (d, g) = (receipt / 3, receipt % 3);
                if g == 0 {
                    prepare_consecutive(&mut chain, binding, d);
                    local(
                        &mut chain,
                        d,
                        [
                            S::MergeWake,
                            S::ReleaseBegin(2),
                            S::Decrement(2),
                            S::ReleaseEnd(2),
                            S::PublishEnd(2),
                        ],
                    );
                    assert_eq!(chain.owner(d).references(), 3);
                    for member in [1, 2] {
                        acquire(&mut chain, binding, d, member);
                    }
                    if d == 0 {
                        step(&mut chain, A::Handoff(0));
                    }
                } else {
                    collect(&mut chain, d, g);
                }
            }
            for teardown in [[0, 1], [1, 0]] {
                let mut path = chain.clone();
                for d in teardown {
                    let detached = !(paths + d).is_multiple_of(2);
                    let pair = if (paths / 2 + d).is_multiple_of(2) {
                        [0, 1]
                    } else {
                        [1, 0]
                    };
                    finish(&mut path, binding, d, detached, pair, 3);
                }
                assert!(path.terminal());
                paths += 1;
            }
        }
        assert_eq!(paths, 80);
        println!("CONSECUTIVE MODEL {binding:?} {paths} composed paths");
    }
}

#[test]
fn merger_cannot_consume_an_earlier_batch_reference() {
    let binding = Binding::CDirect;
    let mut chain = DomainChain::with_consecutive_batches(binding);
    prepare_consecutive(&mut chain, binding, 0);
    let mut prior = chain.owner(0).clone();
    prior.fault = SharedFault::MergeConsumesEarlierBatch;
    let mut corrupt = prior.clone();
    corrupt.apply(S::MergeWake);
    corrupt.check().unwrap();
    assert_eq!(
        prior
            .check_step(S::MergeWake, &corrupt)
            .unwrap_err()
            .invariant,
        "NL-I7"
    );
}

#[test]
fn stale_reference_return_cannot_follow_the_twice_reused_carrier() {
    let binding = Binding::CDirect;
    let mut chain = Chain::new(binding);
    for domain in [0, 1] {
        register(&mut chain, domain, 0);
        copy(&mut chain, binding, domain, 0);
        collect(&mut chain, domain, 0);
        register(&mut chain, domain, 1);
        copy(&mut chain, binding, domain, 1);
        step(&mut chain, A::Handoff(domain));
    }
    register(&mut chain, 2, 0);
    notify(&mut chain, binding, 0, 0, true);
    local(&mut chain, 0, [S::ReleaseBegin(0)]);
    chain.fault = ChainFault::ReturnToLatestOwner;
    let action = A::Local(0, S::Decrement(0));
    let mut wrong = chain.clone();
    wrong.apply(action);
    assert_eq!(
        chain.check_step(action, &wrong).unwrap_err().invariant,
        "NL-I7"
    );
}
