use super::cohort_tests::{perform_call, receive, start_completion};
use super::extended_tests::arm_poll;
use super::*;
use rinzu_runtime::lifetime_trace::Probe;
use rinzu_runtime::native_completion::{
    CompletionPointer, NotifiedNativeCompletion, TransferNativeCompletion,
};
use rinzu_runtime_model::native_lifetime::{
    Protocol, ReadyAction as A, ReadyInput, ReadySelection,
};

fn publish(raw: usize, generation: u64, probe: &Probe) {
    let _sdk = Probe::producer(generation);
    // SAFETY: this operation publishes a pinned carrier once after SDK preparation.
    unsafe {
        NotifiedNativeCompletion::publish_with_lifetime_trace(
            raw as *mut NotifiedNativeCompletion,
            probe,
        );
    }
}

fn initial_selection(probe: &Probe, operations: [OperationId; 3], futures: [u64; 3]) -> usize {
    let events = probe.events();
    let mut model = ReadySelection::new(ReadyInput::Packets);
    for row in &events {
        let parts = row.split(',').collect::<Vec<_>>();
        let id = parts[1].parse::<u64>().unwrap();
        let index = || {
            operations
                .iter()
                .position(|operation| operation.0 == id)
                .unwrap()
        };
        let action = match parts[0] {
            "physical" => A::Publish(index()),
            "owner_packet_read" => A::Observe(index()),
            "owner_inbox_empty" => A::Select,
            "owner_initial_selection" => {
                let selected = futures.iter().position(|&future| future == id).unwrap();
                assert_eq!(model.selection(), Some(selected), "{}", events.join("\n"));
                assert!(model.actions().contains(&A::Finish));
                model.apply(A::Finish);
                return selected;
            }
            _ => continue,
        };
        assert!(
            model.actions().contains(&action),
            "{row}\n{}",
            events.join("\n")
        );
        let before = model.clone();
        model.apply(action);
        before.check_step(action, &model).unwrap();
        model.check().unwrap();
    }
    panic!("Owner selection missing\n{}", events.join("\n"));
}

#[test]
fn native_packet_can_arrive_during_an_owner_receive_turn() {
    let module = Arc::new(
        crate::test_library::compile_library(&rinzu_compiler::SourceFile::new(
            rinzu_compiler::FileId(892),
            "packet-selection.rinzu",
            r#"
native {fn start(g:Int,callback:fn()->CSize)->CSize symbol "cohort_start";}
fn answer()->CSize fallback 0{return CSize(7183);}
async fn main(){let call=start(0,answer);}
"#,
        ))
        .unwrap()
        .ir,
    );
    for late in [0, 1] {
        for after_empty in [false, true] {
            run(&module, late, after_empty);
        }
    }
}

fn run(module: &Arc<IrModule>, late: usize, after_empty: bool) {
    let probe = Probe::new();
    let _trace = Probe::install(Arc::clone(&probe));
    let owner = OwnerValues::new(module).unwrap();
    let shared = Arc::new(Mutex::new(SharedState::default()));
    let mut ops = SourceOperations::new(
        Arc::clone(module),
        Arc::clone(&shared),
        &SourceAsyncOptions::default(),
        &owner,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Arc::new(NativeBindings::default()),
    );
    let carriers = [7199, 7200].map(|result| {
        Box::new(TransferNativeCompletion {
            result,
            ..Default::default()
        })
    });
    let addresses = carriers
        .each_ref()
        .map(|carrier| std::ptr::from_ref(&carrier.completion) as usize);
    let mut start = |i| {
        let raw = addresses[i];
        let generation = 100 + i as u64;
        probe.observe_carrier(raw, generation);
        perform_call(module, &owner, &probe, raw, i);
        // SAFETY: the two provider allocations remain pinned through owner receipt settlement.
        let pointer = unsafe { CompletionPointer::new(raw) };
        start_completion(&shared, &mut ops, &probe, pointer, (generation, 0), true)
    };
    let first = start(0);
    let second = start(1);
    let (poll_storage, poll) = arm_poll(module, &owner, &shared, &mut ops, &probe);
    let futures = [first, second, poll];
    let operations = futures.map(|f| ops.active[&f].0);
    probe.wait("registered", 2);

    poll_storage.publish(&probe);
    probe.wait_for("worker_wake_end", operations[2].0, 1);
    let epoch = ops.completion_signal.epoch();
    let raw = addresses[late];
    let poll_probe = Arc::clone(&probe);
    let mut injected = false;
    ops.after_packet_read = Some(Box::new(move |packet| {
        if let Some(id) = packet {
            poll_probe.record("owner_packet_read", id.0, 0, 0);
        } else {
            poll_probe.record("owner_inbox_empty", 0, 0, 0);
        }
        if !injected && (after_empty == packet.is_none()) {
            injected = true;
            publish(raw, 100 + late as u64, &poll_probe);
            poll_probe.wait_for("worker_wake_end", operations[late].0, 1);
        }
    }));
    let selection_probe = Arc::clone(&probe);
    ops.after_ready_selection = Some(Box::new(move |future| {
        selection_probe.record("owner_initial_selection", future, 0, 0);
    }));
    assert!(ops.receive_one_ready().unwrap());
    ops.after_packet_read = None;
    let selected = initial_selection(&probe, operations, futures);
    assert_eq!(selected, if after_empty { 2 } else { late });
    assert!(ops.completion_signal.epoch() > epoch);
    for (i, f) in futures.into_iter().enumerate() {
        assert_eq!(
            shared.lock().unwrap().completed.contains_key(&f),
            i == selected
        );
    }
    assert!(ops.receive_one_ready().unwrap());
    let other = 1 - late;
    publish(addresses[other], 100 + other as u64, &probe);
    receive(&mut ops, &shared, futures);
    for (i, f) in futures.into_iter().enumerate().take(2) {
        assert!(
            matches!(shared.lock().unwrap().completed[&f], Ok(Value::Scalar(value)) if value.bits() == 7199 + i as u64)
        );
    }
    assert!(ops.is_baseline());
    drop(ops);
    owner.borrow_mut().finish().unwrap();
    println!("READY SELECTION Rust late={late} after_empty={after_empty} selected={selected}");
}
