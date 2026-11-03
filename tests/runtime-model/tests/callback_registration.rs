use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use rinzu_runtime_model::callback_registration::{
    CallbackRegistration, Delivery, RegistrationState, RetainedArgument,
};

#[test]
fn overflow_unregister_and_late_callbacks_release_every_argument_once() {
    const PRODUCERS: usize = 8;
    const PER_PRODUCER: usize = 100;

    let registration = Arc::new(CallbackRegistration::new(4));
    let generation = registration.generation();
    let released = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(PRODUCERS + 2));
    let owner_done = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();

    for _ in 0..PRODUCERS {
        let registration = Arc::clone(&registration);
        let released = Arc::clone(&released);
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            barrier.wait();
            for _ in 0..PER_PRODUCER {
                registration.deliver(generation, RetainedArgument::new(Arc::clone(&released)));
                thread::yield_now();
            }
        }));
    }

    let owner_registration = Arc::clone(&registration);
    let owner_barrier = Arc::clone(&barrier);
    let owner_done_flag = Arc::clone(&owner_done);
    let owner = thread::spawn(move || {
        owner_barrier.wait();
        while !owner_done_flag.load(Ordering::Acquire) {
            owner_registration.complete_active();
            thread::yield_now();
        }
        owner_registration.unregister();
    });

    barrier.wait();
    for producer in threads {
        producer.join().expect("callback producer panicked");
    }
    owner_done.store(true, Ordering::Release);
    owner.join().expect("callback owner panicked");

    assert_eq!(registration.state(), RegistrationState::Closed);
    assert_eq!(registration.native_unregister_count(), 1);
    assert_eq!(registration.outstanding_handlers(), 0);
    assert_eq!(released.load(Ordering::Acquire), PRODUCERS * PER_PRODUCER);

    let late = registration.deliver(generation, RetainedArgument::new(Arc::clone(&released)));
    assert_eq!(late, Delivery::Late);
    assert_eq!(
        released.load(Ordering::Acquire),
        PRODUCERS * PER_PRODUCER + 1
    );
    assert_eq!(registration.native_unregister_count(), 1);
}

#[test]
fn overflow_winner_starts_native_unregister_without_explicit_close() {
    let registration = CallbackRegistration::new(1);
    let generation = registration.generation();
    let released = Arc::new(AtomicUsize::new(0));

    assert_eq!(
        registration.deliver(generation, RetainedArgument::new(Arc::clone(&released))),
        Delivery::Started
    );
    assert_eq!(
        registration.deliver(generation, RetainedArgument::new(Arc::clone(&released))),
        Delivery::Queued
    );
    assert_eq!(
        registration.deliver(generation, RetainedArgument::new(Arc::clone(&released))),
        Delivery::Overflow
    );

    assert_eq!(registration.state(), RegistrationState::Failed);
    assert_eq!(registration.native_unregister_count(), 1);
    assert_eq!(registration.outstanding_handlers(), 0);
    assert_eq!(released.load(Ordering::Acquire), 3);

    registration.unregister();
    assert_eq!(registration.state(), RegistrationState::Closed);
    assert_eq!(registration.native_unregister_count(), 1);
}
