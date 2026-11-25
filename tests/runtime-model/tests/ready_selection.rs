use rinzu_runtime_model::native_lifetime::{
    Protocol, ReadyAction as A, ReadyInput, ReadySelection, explore,
};

#[test]
fn explores_receipt_arrival_during_candidate_and_packet_observation() {
    for input in [ReadyInput::Candidates, ReadyInput::Packets] {
        let report = explore(ReadySelection::new(input), 10_000).unwrap();
        assert_eq!(report.allowed_stalls, 0);
        for witness in [
            "older-publication-outside-the-selected-observations",
            "late-publication-after-an-empty-turn",
        ] {
            assert!(report.witnesses.contains(witness));
        }
        println!(
            "READY SELECTION MODEL {input:?} states={} edges={} terminals={}",
            report.states, report.transitions, report.terminals
        );
    }
}

#[test]
fn publication_cannot_revisit_an_observation_or_overwrite_a_selection() {
    for input in [ReadyInput::Candidates, ReadyInput::Packets] {
        let mut model = ReadySelection::new(input);
        for a in [A::Publish(2), A::Observe(2)] {
            assert!(model.actions().contains(&a));
            model.apply(a);
        }
        if input == ReadyInput::Candidates {
            model.apply(A::Observe(1));
            model.apply(A::Observe(0));
        }
        model.apply(A::Select);
        let before = model.clone();
        model.apply(A::Publish(0));
        assert_eq!(model.selection(), Some(2));
        assert!(!model.observed(0));
        model.check().unwrap();
        before.check_step(A::Publish(0), &model).unwrap();
        model.apply(A::Finish);
        assert!(model.terminal());
    }
}
