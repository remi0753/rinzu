use rinzu_runtime_model::native_lifetime::{
    Decision, DeferredPoll, DeferredPollAction as A, Protocol, explore,
};

fn step(model: &mut DeferredPoll, a: A) {
    assert!(model.actions().contains(&a), "{a:?}");
    let before = model.clone();
    model.apply(a);
    before.check_step(a, model).unwrap();
    model.check().unwrap();
}

#[test]
fn exhausts_the_direct_deferred_and_polling_selection_boundaries() {
    let report = explore(DeferredPoll::default(), 10_000).unwrap();
    assert_eq!(report.allowed_stalls, 0);
    for witness in [
        "sdk-publication-after-poll-selection",
        "two-direct-copies-with-poll-credit-held",
    ] {
        assert!(report.witnesses.contains(witness), "{witness}");
    }
    println!(
        "DEFERRED POLL MODEL {} states, {} edges, {} terminals",
        report.states, report.transitions, report.terminals
    );
}

#[test]
fn late_publication_preserves_selection_and_deferred_decisions() {
    let mut base = DeferredPoll::default();
    for a in [
        A::RejectAdmission(0),
        A::RejectAdmission(1),
        A::Control(0, Decision::Cancelled),
        A::Control(1, Decision::TimedOut),
        A::PollReady,
    ] {
        step(&mut base, a);
    }
    for late in [false, true] {
        let mut model = base.clone();
        if late {
            step(&mut model, A::SelectPoll);
            step(&mut model, A::PublishSdk);
            assert!(!model.actions().contains(&A::DirectCopy(0)));
            step(&mut model, A::SettlePoll);
        } else {
            step(&mut model, A::PublishSdk);
            assert!(!model.actions().contains(&A::SelectPoll));
        }
        step(&mut model, A::DirectCopy(0));
        assert!(!model.actions().contains(&A::RetireCarrier));
        step(&mut model, A::DirectCopy(1));
        assert_eq!(model.poll_has_credit(), !late);
        assert_eq!(model.decision(0), Decision::Cancelled);
        assert_eq!(model.decision(1), Decision::TimedOut);
        step(&mut model, A::RetireCarrier);
        if !late {
            step(&mut model, A::SelectPoll);
            step(&mut model, A::SettlePoll);
        }
        assert!(model.terminal());
    }
}

#[test]
fn explores_deferred_retry_with_a_direct_peer_and_one_credit() {
    let report = explore(DeferredPoll::allowing_retry(), 10_000).unwrap();
    assert_eq!(report.allowed_stalls, 0);
    for witness in [
        "published-after-retry-before-registration",
        "retry-registration-transferred-the-sdk-reference",
        "two-direct-copies-with-poll-credit-held",
    ] {
        assert!(report.witnesses.contains(witness), "{witness}");
    }
    println!(
        "DEFERRED RETRY MODEL {} states, {} edges, {} terminals",
        report.states, report.transitions, report.terminals
    );
}

#[test]
fn retry_cannot_return_its_credit_or_erase_control_at_publication() {
    for early in [false, true] {
        let mut model = DeferredPoll::allowing_retry();
        for a in [
            A::RejectAdmission(0),
            A::RejectAdmission(1),
            A::Control(0, Decision::TimedOut),
            A::Control(1, Decision::Cancelled),
            A::PollReady,
            A::SelectPoll,
        ] {
            step(&mut model, a);
        }
        assert!(!model.actions().contains(&A::Retry(0)));
        step(&mut model, A::SettlePoll);
        step(&mut model, A::Retry(0));
        assert!(!model.actions().contains(&A::Retry(1)));
        let publication = A::PublishSdk;
        let register = A::RegisterRetry(0);
        for a in if early {
            [publication, register]
        } else {
            [register, publication]
        } {
            step(&mut model, a);
        }
        assert_eq!(model.sdk_has_reference(), !early);
        step(&mut model, A::CopyReceipt(0));
        step(&mut model, A::DirectCopy(1));
        assert!(!model.actions().contains(&A::RetireCarrier));
        step(&mut model, A::SettleReceipt(0));
        step(&mut model, A::RetireCarrier);
        assert_eq!(model.decision(0), Decision::TimedOut);
        assert_eq!(model.decision(1), Decision::Cancelled);
        if !early {
            assert!(!model.terminal());
            step(&mut model, A::ReturnSdk);
        }
        assert!(model.terminal());
    }
}
