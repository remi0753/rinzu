use rinzu_runtime_model::native_lifetime::{
    Domain::{First, Second},
    ParallelRegistration, ParallelRegistrationAction as A, Protocol, explore,
};

#[test]
fn explores_two_distinct_registrars_and_the_sdk_exchange() {
    let report = explore(ParallelRegistration::default(), 10_000).unwrap();
    assert_eq!(report.allowed_stalls, 0);
    for witness in [
        "overlapping-candidates",
        "publication-before-both-cas-operations",
        "publication-between-cas-operations-keeps-winner-reference",
        "foreign-cas-returns-only-losing-candidate",
    ] {
        assert!(report.witnesses.contains(witness), "{witness}");
    }
    println!(
        "PARALLEL REGISTRATION MODEL {} states, {} edges, {} terminals",
        report.states, report.transitions, report.terminals
    );
}

#[test]
fn a_foreign_cas_is_rejected_even_if_the_sdk_publishes_before_rollback() {
    for (winner, loser) in [(First, Second), (Second, First)] {
        let mut model = ParallelRegistration::default();
        for a in [
            A::Retain(winner),
            A::Retain(loser),
            A::Cas(winner),
            A::Cas(loser),
            A::Exchange,
            A::Rollback(loser),
        ] {
            assert!(model.actions().contains(&a), "{a:?}");
            let before = model.clone();
            model.apply(a);
            before.check_step(a, &model).unwrap();
            model.check().unwrap();
        }
        assert!(model.terminal() && model.rejected() && model.published());
        assert_eq!(model.captured(), Some(winner));
        assert_eq!([model.references(winner), model.references(loser)], [2, 1]);
    }
}
