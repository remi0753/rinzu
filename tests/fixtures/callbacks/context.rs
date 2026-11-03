// Spike: hidden typed capture context over the unchanged two-argument C entry.
use core::ffi::c_void;
use rinzu_runtime::callback_entry::{CallbackAbiCursor, CallbackFrame, FrameCallback};
use rinzu_runtime::data::{ManagedData, ManagedHandle, ManagedRoots, ManagedSlot, RootLease};
use rinzu_runtime::foreign_queue::ForeignQueue;
use rinzu_runtime::heap::{CollectionKind, DomainHeap};
use rinzu_runtime::scalar::{ScalarKind, ScalarValue};
use rinzu_runtime::value::{ValueSchema, ValueType};
use std::cell::{Cell, RefCell};
use std::sync::Arc;
type Entry = unsafe extern "C" fn(i64, *mut c_void) -> i64;
const USERDATA: usize = 0x1234;
fn scalar(value: u64) -> ManagedSlot {
    ManagedSlot::Scalar(ScalarValue::from_bits(ScalarKind::I64, value).unwrap())
}
struct Owner {
    heap: DomainHeap,
    data: ManagedData,
    roots: ManagedRoots,
    child: Option<RootLease>,
}
impl Owner {
    fn new() -> (Self, ManagedHandle) {
        let heap = DomainHeap::new(950);
        let mut data = ManagedData::new(950);
        data.configure_values(
            &heap,
            Arc::new(
                ValueSchema::new(vec![
                    ValueType::Scalar(ScalarKind::I64),
                    ValueType::State {
                        identity: "Editor".into(),
                        fields: vec![0],
                    },
                ])
                .unwrap(),
            ),
        )
        .unwrap();
        let mut owner = Self {
            heap,
            data,
            roots: ManagedRoots::default(),
            child: None,
        };
        let state = owner
            .data
            .create_composite(&mut owner.heap, 1, None, &[scalar(39)])
            .unwrap();
        (owner, state)
    }
    fn collect(&mut self) {
        self.data
            .retain_roots(&mut self.heap, self.roots.live())
            .unwrap();
        self.heap.collect(CollectionKind::Full).unwrap();
    }
    fn read(&mut self, state: ManagedHandle) -> u64 {
        let ManagedSlot::Scalar(value) = self
            .data
            .project(&mut self.heap, state, 1, None, 0)
            .unwrap()
        else {
            panic!("scalar")
        };
        value.bits()
    }
}
struct Context {
    owner: RefCell<Owner>,
    capture: ManagedHandle,
    entry: Cell<usize>,
    origin: std::thread::ThreadId,
    invocations: Cell<usize>,
}
unsafe extern "C" fn handler(raw: *mut c_void, frame: *const CallbackFrame) -> u64 {
    // SAFETY: the actual entry gates Origin before accessing this pinned context.
    let context = unsafe { &*raw.cast::<Context>() };
    assert_eq!(std::thread::current().id(), context.origin);
    // SAFETY: this is the live exact (i64, void*) native frame.
    let frame = unsafe { &*frame };
    let mut cursor = CallbackAbiCursor::default();
    // SAFETY: fixed public arity remains two, including the explicit userdata.
    let increment = unsafe { frame.word(&mut cursor, Some(ScalarKind::I64)) };
    // SAFETY: the second native argument remains the original userdata word.
    assert_eq!(unsafe { frame.word(&mut cursor, None) }, USERDATA as u64);
    {
        let mut owner = context.owner.borrow_mut();
        owner.collect();
        let value = owner.read(context.capture);
        let owner = &mut *owner;
        owner
            .data
            .set_state_field(
                &mut owner.heap,
                context.capture,
                1,
                0,
                scalar(value + increment),
            )
            .unwrap();
        if owner.child.is_none() {
            owner.child = Some(owner.roots.pin(context.capture).unwrap());
        }
    }
    context.invocations.set(context.invocations.get() + 1);
    if increment == 1 {
        // SAFETY: same admitted C signature/Origin; outer heap borrow was released.
        let entry: Entry = unsafe { std::mem::transmute(context.entry.get()) };
        // SAFETY: immutable explicit userdata token is passed unchanged.
        assert_eq!(unsafe { entry(2, USERDATA as *mut c_void) }, 42);
    }
    context.owner.borrow_mut().read(context.capture)
}
#[test]
fn internal_context_preserves_gc_reentry_foreign_receipt_and_pending_child() {
    let (mut owner, state) = Owner::new();
    let capsule = owner.roots.pin(state).unwrap();
    let mut context = Box::new(Context {
        owner: RefCell::new(owner),
        capture: state,
        entry: Cell::new(0),
        origin: std::thread::current().id(),
        invocations: Cell::new(0),
    });
    // SAFETY: Box and capture root stay pinned through every accepted receipt.
    let entry = unsafe {
        FrameCallback::prepare_deferred(handler, std::ptr::from_mut(&mut *context).cast())
    };
    context.entry.set(entry.entry() as usize);
    // SAFETY: no wake context; Owner and queue outlive the admitted worker.
    let queue = unsafe { ForeignQueue::new(None, std::ptr::null_mut()) };
    queue.attach_main();
    queue.begin_loop(7);
    // SAFETY: validated scalar+userdata-pointer spike only; native-only token is
    // used here to test ABI arity, not to expand production userdata eligibility.
    unsafe { entry.bind_foreign(&queue, 7, true) };
    entry.open();
    for _ in 0..200 {
        context.owner.borrow_mut().collect();
    }
    // SAFETY: entry was prepared for exactly this signature.
    let native: Entry = unsafe { std::mem::transmute(entry.entry()) };
    // SAFETY: same Origin, live capsule and unchanged native arguments.
    assert_eq!(unsafe { native(1, USERDATA as *mut c_void) }, 42);
    let address = entry.entry() as usize;
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            // SAFETY: eligible admitted signature, pinned context/queue, native words.
            let native: Entry = unsafe { std::mem::transmute(address) };
            // SAFETY: the admitted entry and pinned context/queue outlive this scoped
            // worker; USERDATA is an opaque native word and is never dereferenced.
            unsafe { native(3, USERDATA as *mut c_void) }
        });
        while queue.stats().queued == 0 {
            std::thread::yield_now();
        }
        assert!(!entry.close());
        assert!(!entry.settled());
        context.owner.borrow_mut().collect();
        assert!(rinzu_runtime::callback_entry::drain_foreign_one(&queue));
        assert_eq!(worker.join().unwrap(), 45);
    });
    while !entry.settled() {
        std::thread::yield_now();
    }
    assert!(queue.stats().is_baseline());
    queue.end_loop(7);
    let mut owner = context.owner.borrow_mut();
    owner.roots.unpin(capsule).unwrap();
    owner.collect();
    assert_eq!(owner.read(state), 45); // child still owns the capture after Close.
    let child = owner.child.take().unwrap();
    owner.roots.unpin(child).unwrap();
    owner.collect();
    assert_eq!(
        (
            owner.heap.object_count(),
            owner.data.root_count(),
            owner.roots.lease_count()
        ),
        (0, 0, 0)
    );
    assert_eq!(context.invocations.get(), 3);
    println!("PASS capture: GC200 nested2 foreign1 close-before-receipt child-root ABI2 final0");
}
#[test]
fn a_context_word_without_root_does_not_keep_the_environment_alive() {
    let (mut owner, state) = Owner::new();
    owner.collect();
    assert!(
        owner
            .data
            .project(&mut owner.heap, state, 1, None, 0)
            .is_err()
    );
    assert_eq!(owner.heap.object_count(), 0);
    println!("PASS counterexample: context-address alone is not a GC root");
}
