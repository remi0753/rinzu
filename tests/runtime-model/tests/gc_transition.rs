use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use rinzu_runtime_model::gc_transition::{DomainHeap, GcError, ThreadState};

#[test]
fn native_safe_callback_and_moving_gc_preserve_stable_handles() {
    let mut main_heap = DomainHeap::new(4);
    let object = main_heap.allocate(42).expect("allocate root");
    let handle = main_heap
        .create_handle(object)
        .expect("create stable handle");

    main_heap.enter_native_safe().expect("publish native roots");
    assert_eq!(main_heap.collect(), Err(GcError::CollectionDeferred));

    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut worker_heap = DomainHeap::new(3);
        for value in 0..1_000 {
            worker_heap.allocate(value).expect("worker allocation");
        }
        done_tx
            .send(worker_heap.collection_count())
            .expect("send worker GC count");
    });

    let worker_collections = done_rx.recv().expect("worker should finish independently");
    assert!(worker_collections > 0);

    main_heap
        .begin_owner_sync_callback()
        .expect("same-thread callback entry");
    assert_eq!(main_heap.state(), ThreadState::CallbackManaged);
    for value in 0..16 {
        main_heap.allocate(value).expect("callback allocation");
    }
    assert!(main_heap.collection_count() > 0);
    assert_eq!(main_heap.read_handle(handle), Ok(42));
    main_heap
        .end_owner_sync_callback()
        .expect("callback returns to native");
    main_heap.leave_native_safe().expect("native frame returns");
    main_heap.release_handle(handle).expect("release handle");

    worker.join().expect("worker GC thread panicked");
}

#[test]
fn wrong_thread_cannot_collect_or_move_a_domain_heap() {
    let heap = Arc::new(Mutex::new(DomainHeap::new(8)));
    heap.lock()
        .expect("main heap lock")
        .allocate(7)
        .expect("main allocation");

    let foreign = Arc::clone(&heap);
    let result = thread::spawn(move || foreign.lock().expect("foreign lock").collect())
        .join()
        .expect("foreign thread panicked");

    assert_eq!(result, Err(GcError::WrongThread));
}
