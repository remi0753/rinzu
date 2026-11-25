use rinzu_runtime_model::native_lifetime::{
    Binding, FailureKind, SharedFault, SharedTarget, SharedTopology, explore,
};
#[test]
fn exhausts_shared_references_and_overlapping_generations() {
    for binding in [Binding::CDirect, Binding::CJoined, Binding::RustChannel] {
        for topology in [
            SharedTopology::Carriers,
            SharedTopology::Reuse,
            SharedTopology::Mixed,
        ] {
            let r = explore(SharedTarget::new(binding, topology), 5_000_000)
                .unwrap_or_else(|e| panic!("{e}"));
            println!(
                "{} states={} edges={} terminals={} depth={}",
                r.profile, r.states, r.transitions, r.terminals, r.maximum_shortest_depth
            );
            assert!(r.terminals > 0);
            assert_eq!(r.allowed_stalls, 0);
            for w in [
                "more-than-two-target-references",
                "two-producers-outlive-runtime",
                "sdk-reference-outlives-shared-environment",
            ] {
                assert!(r.witnesses.contains(w), "{} missing {w}", r.profile);
            }
            if topology == SharedTopology::Mixed {
                assert!(r.witnesses.contains("four-target-references"));
            }
            if topology.replacement().is_some() {
                for w in [
                    "old-sdk-reference-survives-storage-reuse",
                    "reuse-before-old-publisher-return",
                ] {
                    assert!(r.witnesses.contains(w), "{} missing {w}", r.profile);
                }
            }
        }
    }
}
#[test]
fn rejects_shared_reference_and_generation_faults() {
    for (fault, invariant) in [
        (SharedFault::FreeAfterOneRelease, "NL-I7"),
        (SharedFault::DetachAfterOneCopy, "NL-I5"),
        (SharedFault::FreeEnvironmentOnCancel, "NL-I3"),
        (SharedFault::ForgetReferenceOnReuse, "NL-I7"),
        (SharedFault::ReadReplacementValue, "NL-I8"),
        (SharedFault::OldControlTouchesReplacement, "NL-I8"),
        (SharedFault::CandidateDropConsumesOtherProducer, "NL-I7"),
        (SharedFault::LateNotifyTouchesCarrier, "NL-I4"),
    ] {
        let mut s = SharedTarget::new(Binding::CDirect, SharedTopology::Reuse);
        s.fault = fault;
        let e = explore(s, 5_000_000).expect_err("unsafe shared lifetime passed");
        assert_eq!(e.kind, FailureKind::Invariant, "{e}");
        assert!(e.reason.starts_with(invariant), "{e}");
        println!(
            "SHARED MUTANT {fault:?} {} {}",
            e.reason,
            e.trace.join(" -> ")
        );
    }
}

#[test]
fn coalesced_notification_covers_both_ready_receipt_orders() {
    for binding in [Binding::CDirect, Binding::RustChannel] {
        let r = explore(
            SharedTarget::new(binding, SharedTopology::Coalesced),
            500_000,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        println!(
            "{} states={} edges={} terminals={} depth={}",
            r.profile, r.states, r.transitions, r.terminals, r.maximum_shortest_depth
        );
        assert!(r.terminals > 0);
        assert_eq!(r.allowed_stalls, 0);
        for witness in [
            "leader-settled-first",
            "covered-receipt-settled-first",
            "duplicate-released-while-batch-target-retained",
        ] {
            assert!(
                r.witnesses.contains(witness),
                "{} missing {witness}",
                r.profile
            );
        }
    }
}

#[test]
fn rejects_coalesced_reference_loss_and_early_notification() {
    for fault in [
        SharedFault::MergeConsumesLeader,
        SharedFault::NotifyBeforeCoveredPublication,
    ] {
        let mut state = SharedTarget::new(Binding::CDirect, SharedTopology::Coalesced);
        state.fault = fault;
        let e = explore(state, 100_000).expect_err("unsafe batch passed");
        assert_eq!(e.kind, FailureKind::Invariant, "{e}");
        assert!(e.reason.starts_with("NL-I7"), "{e}");
        println!(
            "SHARED MUTANT {fault:?} {} {}",
            e.reason,
            e.trace.join(" -> ")
        );
    }
}
