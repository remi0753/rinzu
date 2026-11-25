use super::{c_fixture_trace_on, with_c_binary};
use rinzu_runtime_model::native_lifetime::{Binding, Domain, replay_joined_handoff, reuse_orders};

#[test]
fn c_reuses_a_carrier_while_the_origin_polling_helper_is_still_waking() {
    with_c_binary("joined", "joined_domain_handoff.c", |binary, sanitized| {
        for order in reuse_orders() {
            let digits: String = order
                .into_iter()
                .map(|g| char::from(b'0' + u8::try_from(g).unwrap()))
                .collect();
            for first in Domain::ALL {
                for early in [false, true] {
                    let events = c_fixture_trace_on(
                        binary,
                        &[
                            digits.clone(),
                            first.index().to_string(),
                            usize::from(early).to_string(),
                        ],
                        Binding::CJoined,
                    );
                    let replay = |rows: &[String]| replay_joined_handoff(rows, 100_000);
                    let report = replay(&events)
                        .unwrap_or_else(|error| panic!("{error}\n{}", events.join("\n")));
                    assert_eq!(report.settlement_order, order);
                    assert_eq!(report.destruction_order, [first, first.other()]);
                    assert_eq!(report.replacement_published_before_registration, early);
                    if order == [0, 1, 2, 3] {
                        for prefix in [
                            "0,helper_identity,",
                            "0,context_identity,",
                            "0,helper_joined,",
                            "0,joined_producer_refs,",
                        ] {
                            let mut missing = events.clone();
                            missing.remove(
                                events
                                    .iter()
                                    .position(|row| row.starts_with(prefix))
                                    .unwrap(),
                            );
                            assert!(replay(&missing).is_err(), "accepted missing {prefix}");
                        }
                        let mut wrong = events.clone();
                        let index = events
                            .iter()
                            .position(|row| row.starts_with("1,joined_handoff,"))
                            .unwrap();
                        wrong[index] = "1,joined_handoff,0,32,0".into();
                        assert!(replay(&wrong).is_err(), "accepted a foreign carrier");
                    }
                    println!(
                        "JOINED HANDOFF C sanitized={sanitized} {order:?} first={first:?} early={early} events={}",
                        events.len()
                    );
                }
            }
        }
    });
}
