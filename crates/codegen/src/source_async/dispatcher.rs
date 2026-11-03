//! Independent Main turn context: no parked task/frame or heap borrow is held
//! through a RunLoop hook. The Owner pins this box and all registries are weak.
use super::*;
pub(super) struct Context {
    owner: Weak<RefCell<OwnerValues>>,
    shared: Arc<Mutex<SharedState>>,
    tasks: Weak<RefCell<BTreeMap<u64, SourceTask>>>,
    operations: Weak<RefCell<SourceOperations>>,
}
pub(super) fn connect(
    owner: &Rc<RefCell<OwnerValues>>,
    shared: &Arc<Mutex<SharedState>>,
    tasks: &Rc<RefCell<BTreeMap<u64, SourceTask>>>,
    operations: &Rc<RefCell<SourceOperations>>,
) {
    let Some(dispatcher) = owner.borrow().dispatcher.clone() else {
        return;
    };
    let mut context = Box::new(Context {
        owner: Rc::downgrade(owner),
        shared: Arc::clone(shared),
        tasks: Rc::downgrade(tasks),
        operations: Rc::downgrade(operations),
    });
    // SAFETY: Owner pins the independent context until dispatcher destruction;
    // no Rc cycle retains the Owner, and hooks never unwind through native code.
    unsafe {
        dispatcher.set_hooks(
            step,
            ready,
            can_enter,
            std::ptr::from_mut(&mut *context).cast(),
        )
    };
    owner.borrow_mut().dispatcher_context = Some(context);
}
unsafe fn context<'a>(raw: *mut core::ffi::c_void) -> &'a Context {
    if raw.is_null() {
        data::native_fatal(86);
    }
    // SAFETY: runtime registration pins this box until its hooks are removed.
    unsafe { &*raw.cast::<Context>() }
}
fn ready_task(
    context: &Context,
    tasks: &RefCell<BTreeMap<u64, SourceTask>>,
    cleanup: bool,
) -> Option<u64> {
    let shared = context
        .shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let tasks = tasks.try_borrow().ok()?;
    tasks.iter().find_map(|(future, task)| {
        (task.closing.is_some() == cleanup && task.ready_event(&shared).is_some())
            .then_some(*future)
    })
}
unsafe extern "C" fn can_enter(raw: *mut core::ffi::c_void) -> i32 {
    // SAFETY: only registered Owner hooks receive this pinned pointer.
    let context = unsafe { context(raw) };
    context.owner.upgrade().is_some_and(|owner| {
        owner
            .try_borrow()
            .is_ok_and(|owner| owner.dispatcher_can_enter())
    }) as i32
}
unsafe extern "C" fn ready(raw: *mut core::ffi::c_void, lane: u32) -> i32 {
    // SAFETY: only registered Owner hooks receive this pinned pointer.
    let context = unsafe { context(raw) };
    let task = context
        .tasks
        .upgrade()
        .is_some_and(|tasks| ready_task(context, &tasks, lane == 0).is_some());
    let packet = lane == 1
        && context.operations.upgrade().is_some_and(|operations| {
            operations
                .try_borrow()
                .is_ok_and(|operations| operations.has_ready_completion())
        });
    (task || packet) as i32
}
unsafe extern "C" fn step(raw: *mut core::ffi::c_void, lane: u32) -> i32 {
    // SAFETY: only registered Owner hooks receive this pinned pointer.
    let context = unsafe { context(raw) };
    let Some(owner) = context.owner.upgrade() else {
        data::native_fatal(86);
    };
    let scope = owner.borrow().dispatcher_scope();
    let outcome = (|| -> Result<bool> {
        let tasks = context
            .tasks
            .upgrade()
            .ok_or_else(|| anyhow!("Main continuation registry ended"))?;
        if let Some(future) = ready_task(context, &tasks, lane == 0) {
            let task = tasks
                .borrow_mut()
                .remove(&future)
                .ok_or_else(|| anyhow!("ready Main continuation disappeared"))?;
            let event = task
                .ready_event(&context.shared.lock().unwrap_or_else(|p| p.into_inner()))
                .ok_or_else(|| anyhow!("ready Main continuation lost its event"))?;
            park_continuation(&tasks, task, event)?;
            return Ok(true);
        }
        if lane == 1 {
            let operations = context
                .operations
                .upgrade()
                .ok_or_else(|| anyhow!("Main operation registry ended"))?;
            return operations.borrow_mut().receive_one_ready();
        }
        Ok(false)
    })();
    drop(scope);
    match outcome {
        Ok(did) => did as i32,
        Err(_) => data::native_fatal(86),
    }
}
