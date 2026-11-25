use rinzu_runtime_model::native_lifetime::{
    Completion, CompletionAction as A, Decision, Protocol, ReceiptStage,
};

#[test]
fn model_replays_shared_completion_contract_vectors() {
    for line in include_str!("../../conformance/native-lifetime-vectors.csv")
        .lines()
        .skip(1)
    {
        let fields: Vec<_> = line.split(',').collect();
        assert_eq!(fields.len(), 6);
        let mut m = Completion::new(3, 1, 1, false, true);
        let step = |m: &mut Completion, a| {
            let before = m.clone();
            m.apply(a);
            before.check_step(a, m).unwrap();
            m.check().unwrap();
        };
        step(&mut m, A::Admit(0));
        let publish_and_collect = |m: &mut Completion| {
            for a in [
                A::Write,
                A::Publish,
                A::BeginCopy(0),
                A::FinishCopy(0),
                A::Settle(0),
            ] {
                step(m, a);
            }
        };
        if fields[2] == "1" {
            publish_and_collect(&mut m);
        }
        let control = if fields[1] == "1" {
            Decision::Cancelled
        } else {
            Decision::TimedOut
        };
        assert_eq!(m.control(0, control), fields[3] == "1", "{}", fields[0]);
        if fields[2] == "0" {
            assert_eq!(m.stage(0), ReceiptStage::Waiting);
            step(&mut m, A::EndControl(0));
            publish_and_collect(&mut m);
        }
        let expected = match fields[4] {
            "cancelled" => Decision::Cancelled,
            "timed_out" => Decision::TimedOut,
            "completed" => Decision::Completed,
            _ => panic!("bad vector"),
        };
        assert_eq!(m.decision(0), expected, "{}", fields[0]);
        assert!(!m.control(0, control));
        // Scalar bit-level transfer is exercised by the C/Rust adapters; this
        // abstraction copies a generation token, not an i64 representation.
        let _: i64 = fields[5].parse().unwrap();
    }
}
