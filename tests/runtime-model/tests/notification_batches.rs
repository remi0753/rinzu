use rinzu_runtime_model::native_lifetime::{
    Binding, NotificationAction as A, NotificationBatches, Protocol, explore,
};

#[test]
fn explores_three_merged_batches_with_unordered_receipts() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let report = explore(NotificationBatches::<3>::new(binding), 2_000_000).unwrap();
        assert_eq!(report.allowed_stalls, 0);
        assert!(report.witnesses.contains("notify-after-detach"));
        assert!(
            report
                .witnesses
                .contains("notify-before-runtime-reference-return")
        );
        assert!(
            report
                .witnesses
                .contains("runtime-can-return-the-last-reference")
        );
        assert!(
            report
                .witnesses
                .contains("all-batches-retain-independent-leaders")
        );
        assert_eq!(
            report
                .witnesses
                .contains("concurrent-attached-notifications"),
            binding == Binding::RustChannel
        );
        assert_eq!(
            report
                .witnesses
                .contains("detached-during-channel-notification"),
            binding == Binding::RustChannel
        );
        println!(
            "BATCH LEASE MODEL {binding:?} states={} edges={} terminals={}",
            report.states, report.transitions, report.terminals
        );
    }
}

#[test]
fn an_earlier_batch_cannot_pay_for_a_later_duplicate() {
    let mut model = NotificationBatches::<3>::new(Binding::RustChannel);
    for i in 0..3 {
        for member in 0..2 {
            model.apply(A::Capture(i, member));
        }
    }
    let mut wrong = model.clone();
    wrong.apply(A::Merge(0));
    wrong.check().unwrap();
    assert!(model.check_step(A::Merge(2), &wrong).is_err());
}

#[test]
fn detach_preserves_the_runtime_reference_and_every_batch_lease() {
    let mut attached = NotificationBatches::<1>::new(Binding::CDirect);
    for member in 0..2 {
        attached.apply(A::Capture(0, member));
        attached.apply(A::Settle(0, member));
    }
    attached.apply(A::Merge(0));
    assert!(attached.actions().contains(&A::Detach));
    assert!(!attached.actions().contains(&A::ReturnRuntime));
    let mut detached = attached.clone();
    detached.apply(A::Detach);
    assert_eq!(detached.references(), 2);
    assert!(!detached.actions().contains(&A::Destroy));
    let mut both = detached.clone();
    both.apply(A::ReturnRuntime);
    both.check().unwrap();
    assert!(attached.check_step(A::Detach, &both).is_err());
}
