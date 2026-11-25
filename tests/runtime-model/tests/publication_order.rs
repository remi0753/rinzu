//! Bounded happens-before obligations for one notified scalar result.
//!
//! Sequenced-before and atomic release/acquire read-from are the only edges
//! here. A relaxed read-from is deliberately absent from happens-before.
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
enum Event {
    InitTarget,
    Install,
    WritePayload,
    DiagnosticPrefix,
    Exchange,
    SdkReadTarget,
    AcquirePublication,
    Copy,
    PublishReceipt,
    ReceiveReceipt,
    Observe,
}

const EVENTS: usize = Event::Observe as usize + 1;

#[derive(Debug)]
struct Order([[bool; EVENTS]; EVENTS]);

impl Order {
    fn insert(&mut self, before: Event, after: Event) {
        self.0[before as usize][after as usize] = true;
    }

    fn close(&mut self) {
        for via in 0..EVENTS {
            for before in 0..EVENTS {
                for after in 0..EVENTS {
                    self.0[before][after] |= self.0[before][via] && self.0[via][after];
                }
            }
        }
    }

    fn precedes(&self, before: Event, after: Event) -> bool {
        self.0[before as usize][after as usize]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fault {
    None,
    InstallWithoutRelease,
    CaptureWithoutAcquire,
    PublishWithoutRelease,
    ObserveWithoutAcquire,
    PrefixAuthorizesCopy,
    ReceiptWithoutRelease,
    ReceiveWithoutAcquire,
}

fn publication_order(captured: bool, fault: Fault) -> Order {
    use Event::*;
    let mut order = Order([[false; EVENTS]; EVENTS]);
    for (before, after) in [
        (InitTarget, Install),
        (WritePayload, DiagnosticPrefix),
        (DiagnosticPrefix, Exchange),
        (Exchange, SdkReadTarget),
        (AcquirePublication, Copy),
        (Copy, PublishReceipt),
        (ReceiveReceipt, Observe),
    ] {
        order.insert(before, after);
    }
    // The final exchange can read Empty or a target installed by the registrar.
    if captured
        && !matches!(
            fault,
            Fault::InstallWithoutRelease | Fault::CaptureWithoutAcquire
        )
    {
        order.insert(Install, Exchange);
    }
    if !matches!(
        fault,
        Fault::PublishWithoutRelease | Fault::ObserveWithoutAcquire
    ) {
        let read_from = if fault == Fault::PrefixAuthorizesCopy {
            DiagnosticPrefix
        } else {
            Exchange
        };
        order.insert(read_from, AcquirePublication);
    }
    if !matches!(
        fault,
        Fault::ReceiptWithoutRelease | Fault::ReceiveWithoutAcquire
    ) {
        order.insert(PublishReceipt, ReceiveReceipt);
    }
    order.close();
    order
}

#[test]
fn acquire_receipts_order_payload_and_target_publication() {
    use Event::*;
    for captured in [false, true] {
        let order = publication_order(captured, Fault::None);
        assert_eq!(order.precedes(InitTarget, SdkReadTarget), captured);
        for before in [WritePayload, Exchange, Copy] {
            assert!(order.precedes(before, Observe));
        }
        assert!(order.precedes(Exchange, Copy));
    }
}

#[test]
fn weakening_either_half_of_an_ordering_edge_loses_its_guarantee() {
    use Event::*;
    for (fault, before, after) in [
        (Fault::InstallWithoutRelease, InitTarget, SdkReadTarget),
        (Fault::CaptureWithoutAcquire, InitTarget, SdkReadTarget),
        (Fault::PublishWithoutRelease, WritePayload, Copy),
        (Fault::ObserveWithoutAcquire, WritePayload, Copy),
        (Fault::ReceiptWithoutRelease, Copy, Observe),
        (Fault::ReceiveWithoutAcquire, Copy, Observe),
        (Fault::PrefixAuthorizesCopy, Exchange, Copy),
    ] {
        let order = publication_order(true, fault);
        assert!(
            !order.precedes(before, after),
            "{fault:?} ordered {before:?} before {after:?}"
        );
    }
    // An acquire of the diagnostic store may order the payload, but still
    // permits copying and retiring storage before the producer's final access.
    let diagnostic = publication_order(false, Fault::PrefixAuthorizesCopy);
    assert!(diagnostic.precedes(WritePayload, Copy));
    assert!(!diagnostic.precedes(Exchange, Copy));
}
