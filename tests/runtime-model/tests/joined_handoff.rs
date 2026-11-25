use rinzu_runtime_model::native_lifetime::{
    ComposedAction as C, Domain, JoinedAction as J, JoinedHandoff, JoinedHandoffAction as A,
    Protocol, RegistrationAction as R, reuse_orders,
};

fn step(model: &mut JoinedHandoff, action: A) {
    assert!(model.actions().contains(&action), "{action:?}");
    let before = model.clone();
    model.apply(action);
    before.check_step(action, model).unwrap();
    model.check().unwrap();
}

fn local(model: &mut JoinedHandoff, d: Domain, actions: impl IntoIterator<Item = J>) {
    for a in actions {
        step(model, A::Local(d, a));
    }
}

fn primary(model: &mut JoinedHandoff, d: Domain, actions: impl IntoIterator<Item = C>) {
    local(
        model,
        d,
        actions.into_iter().map(|a| J::Primary(R::Product(a))),
    );
}

fn ready(model: &mut JoinedHandoff, d: Domain, early: bool) {
    if early {
        primary(model, d, [C::Write, C::Prefix, C::Publish]);
    }
    primary(model, d, [C::Admit(0)]);
    local(
        model,
        d,
        [R::Begin(0), R::Retain(0), R::CompareExchange(0)]
            .into_iter()
            .map(J::Primary),
    );
    if early {
        local(model, d, [J::Primary(R::Rollback(0))]);
    }
    local(
        model,
        d,
        [J::Primary(R::Return(0)), J::AdmitAux, J::BindAux],
    );
    if !early {
        primary(model, d, [C::Write, C::Prefix, C::Publish]);
    }
    local(
        model,
        d,
        [J::PublishAux, J::CopyAux, J::ClaimAux, J::PhysicalAux],
    );
    primary(
        model,
        d,
        [C::BeginCopy(0), C::Copy(0), C::Claim(0), C::Physical(0)],
    );
    local(model, d, [J::HelperWakeBegin]);
}

fn finish(model: &mut JoinedHandoff, d: Domain, early: bool) {
    primary(model, d, [C::Release(0)]);
    local(model, d, [J::ReleaseAux]);
    primary(model, d, [C::Shutdown]);
    local(model, d, [J::HelperWakeEnd]);
    primary(
        model,
        d,
        [C::Quiescent, C::Join, C::Detach, C::RuntimeReferenceRelease],
    );
    if early {
        primary(model, d, [C::TargetFree, C::EnvironmentFree]);
    } else {
        primary(
            model,
            d,
            [
                C::EnvironmentFree,
                C::NotifyBegin,
                C::NotifyEnd,
                C::ProducerRelease,
                C::TargetFree,
            ],
        );
    }
}

#[test]
fn exhausts_the_joined_handoff_factors() {
    let [origin, replacement] = JoinedHandoff::verify(500_000).unwrap();
    assert_eq!(origin.allowed_stalls + replacement.allowed_stalls, 0);
    assert!(
        origin
            .witnesses
            .contains("storage-reused-with-old-helper-in-wake")
    );
    println!(
        "JOINED HANDOFF MODEL origin {} states, {} edges, {} terminals; replacement {} states, {} edges, {} terminals",
        origin.states,
        origin.transitions,
        origin.terminals,
        replacement.states,
        replacement.transitions,
        replacement.terminals,
    );
}

#[test]
fn recovers_in_any_legal_order_after_handoff_during_a_helper_wake() {
    let mut count = 0;
    for order in reuse_orders() {
        for first in Domain::ALL {
            for early in [false, true] {
                let mut model = JoinedHandoff::default();
                ready(&mut model, Domain::First, false);
                for receipt in order {
                    let d = Domain::ALL[receipt / 2];
                    if receipt % 2 == 0 {
                        primary(
                            &mut model,
                            d,
                            [C::CloseAdmission, C::Settle(0), C::RetireCarrier],
                        );
                        if receipt == 0 {
                            step(&mut model, A::Handoff);
                            ready(&mut model, Domain::Second, early);
                        }
                    } else {
                        local(&mut model, d, [J::SettleAux, J::RetireAux]);
                    }
                }
                finish(&mut model, first, first == Domain::Second && early);
                let other = first.other();
                finish(&mut model, other, other == Domain::Second && early);
                assert!(model.terminal());
                count += 1;
            }
        }
    }
    assert_eq!(count, 32);
    println!("JOINED HANDOFF MODEL {count} composed paths");
}
