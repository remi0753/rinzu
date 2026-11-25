//! Opt-in test observer. No observer or scheduling hooks exist in default builds.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Default)]
struct State {
    events: Vec<String>,
    blocked: HashSet<&'static str>,
    blocked_objects: HashSet<(&'static str, u64)>,
    carriers: HashMap<usize, u64>,
    futures: HashMap<u64, (u64, usize)>,
    operations: HashMap<u64, u64>,
}
thread_local! { static CALL: Cell<u64> = const { Cell::new(0) }; }
thread_local! { static PRODUCER: Cell<u64> = const { Cell::new(0) }; }
thread_local! { static CURRENT: RefCell<Option<Arc<Probe>>> = const { RefCell::new(None) }; }
pub struct Scope {
    previous: Option<Arc<Probe>>,
    _owner: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Drop for Scope {
    fn drop(&mut self) {
        CURRENT.with(|p| *p.borrow_mut() = self.previous.take());
    }
}
/// A closure capture whose final destruction observes the user wake environment.
pub struct Environment(pub Arc<Probe>);
impl Drop for Environment {
    fn drop(&mut self) {
        self.0.record("env_free", 0, 0, 0);
    }
}
pub struct CallScope(u64);
impl Drop for CallScope {
    fn drop(&mut self) {
        CALL.set(self.0);
    }
}
pub struct ProducerScope(u64);
impl Drop for ProducerScope {
    fn drop(&mut self) {
        PRODUCER.set(self.0);
    }
}
#[derive(Default)]
pub struct Probe {
    state: Mutex<State>,
    changed: Condvar,
    domain: Option<(Arc<DomainTrace>, usize)>,
}

/// One ordered test log for separately owned runtime notification Domains.
#[derive(Default)]
pub struct DomainTrace(Mutex<Vec<String>>);

impl DomainTrace {
    #[must_use]
    pub fn probe(self: &Arc<Self>, domain: usize) -> Arc<Probe> {
        Arc::new(Probe {
            domain: Some((Arc::clone(self), domain)),
            ..Probe::default()
        })
    }

    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Probe {
    #[must_use]
    pub fn install(probe: Arc<Self>) -> Scope {
        Scope {
            previous: CURRENT.with(|p| p.replace(Some(probe))),
            _owner: std::marker::PhantomData,
        }
    }
    #[must_use]
    pub fn current() -> Option<Arc<Self>> {
        CURRENT.with(|p| p.borrow().clone())
    }
    pub fn emit(kind: &'static str, a: i64) {
        if let Some(p) = Self::current() {
            p.record(kind, 0, a, 0);
        }
    }
    #[must_use]
    pub fn call(id: u64) -> CallScope {
        CallScope(CALL.replace(id))
    }
    #[must_use]
    pub fn call_identity() -> u64 {
        CALL.get()
    }
    pub fn bind_future(&self, future: u64, generation: u64, index: usize) {
        assert!(
            self.state
                .lock()
                .unwrap()
                .futures
                .insert(future, (generation, index))
                .is_none()
        );
        self.record(
            "future_bind",
            future,
            i64::try_from(generation).unwrap(),
            i64::try_from(index).unwrap(),
        );
    }
    pub fn future_event(&self, kind: &'static str, future: u64, arg: i64) {
        let tagged = self.state.lock().unwrap().futures.contains_key(&future);
        self.record(kind, if tagged { future } else { 0 }, arg, 0);
    }
    pub fn link_future(&self, operation: u64, future: u64) {
        let tagged = self.state.lock().unwrap().futures.contains_key(&future);
        if tagged {
            assert!(
                self.state
                    .lock()
                    .unwrap()
                    .operations
                    .insert(operation, future)
                    .is_none()
            );
            self.record("receipt_link", operation, i64::try_from(future).unwrap(), 0);
        }
    }
    pub(crate) fn bind_wake(&self, operation: u64, address: usize) {
        if self.domain.is_some()
            || self
                .state
                .lock()
                .unwrap()
                .operations
                .contains_key(&operation)
        {
            self.record("wake_bind", operation, i64::try_from(address).unwrap(), 0);
        }
    }
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub(crate) fn domain_event(&self, kind: &'static str, id: u64, a: usize, b: usize) {
        if self.domain.is_some() {
            self.record(
                kind,
                id,
                i64::try_from(a).unwrap(),
                i64::try_from(b).unwrap(),
            );
        }
    }
    pub fn record(&self, kind: &'static str, id: u64, a: i64, b: i64) {
        let id = if id == 0
            && CALL.get() != 0
            && matches!(
                kind,
                "call_enter"
                    | "sdk_start"
                    | "callback_admit"
                    | "callback_begin"
                    | "callback_return"
                    | "call_return"
                    | "call_close"
                    | "call_settle"
                    | "call_read"
                    | "handoff"
                    | "call_env_free"
            ) {
            CALL.get()
        } else {
            id
        };
        let id = if matches!(
            kind,
            "publish_begin"
                | "publish_end"
                | "sdk_prefix"
                | "sdk_exchange"
                | "notify_enter"
                | "notify_ready"
                | "notify_leave"
                | "producer_release"
                | "producer_released"
                | "producer_refs"
                | "context_touch"
        ) && PRODUCER.get() != 0
        {
            PRODUCER.get()
        } else {
            id
        };
        let mut s = self.state.lock().unwrap();
        let line = format!("{kind},{id},{a},{b}");
        if let Some((timeline, domain)) = &self.domain {
            timeline.0.lock().unwrap().push(format!("{domain},{line}"));
        }
        s.events.push(line);
        self.changed.notify_all();
        while s.blocked.contains(kind) || s.blocked_objects.contains(&(kind, id)) {
            s = self.changed.wait(s).unwrap();
        }
    }
    #[must_use]
    pub fn producer(id: u64) -> ProducerScope {
        ProducerScope(PRODUCER.replace(id))
    }
    pub fn observe_carrier(&self, address: usize, generation: u64) {
        self.state
            .lock()
            .unwrap()
            .carriers
            .insert(address, generation);
    }
    pub(crate) fn bind_receipt(&self, operation: u64, address: usize) {
        let generation = self.state.lock().unwrap().carriers.get(&address).copied();
        if let Some(generation) = generation {
            self.domain_event("receipt_carrier", operation, address, 0);
            self.record("receipt", operation, i64::try_from(generation).unwrap(), 0);
        }
    }
    pub fn block_for(&self, kind: &'static str, id: u64) {
        self.state
            .lock()
            .unwrap()
            .blocked_objects
            .insert((kind, id));
    }
    pub fn release_for(&self, kind: &'static str, id: u64) {
        self.state
            .lock()
            .unwrap()
            .blocked_objects
            .remove(&(kind, id));
        self.changed.notify_all();
    }
    pub fn wait_for(&self, kind: &str, id: u64, count: usize) {
        let s = self.state.lock().unwrap();
        let (guard, result) = self
            .changed
            .wait_timeout_while(s, Duration::from_secs(30), |s| {
                s.events
                    .iter()
                    .filter(|e| e.starts_with(&format!("{kind},{id},")))
                    .count()
                    < count
            })
            .unwrap();
        drop(guard);
        assert!(
            !result.timed_out(),
            "lifetime event {kind}/{id} did not arrive"
        );
    }
    pub fn block(&self, kind: &'static str) {
        self.state.lock().unwrap().blocked.insert(kind);
    }
    pub fn release(&self, kind: &'static str) {
        self.state.lock().unwrap().blocked.remove(kind);
        self.changed.notify_all();
    }
    /// # Panics
    /// Panics if the observed boundary is not reached within ten seconds.
    pub fn wait(&self, kind: &str, count: usize) {
        let s = self.state.lock().unwrap();
        let (guard, result) = self
            .changed
            .wait_timeout_while(s, Duration::from_secs(10), |s| {
                s.events
                    .iter()
                    .filter(|e| e.starts_with(&format!("{kind},")))
                    .count()
                    < count
            })
            .unwrap();
        drop(guard);
        assert!(!result.timed_out(), "lifetime event {kind} did not arrive");
    }
    #[must_use]
    pub fn events(&self) -> Vec<String> {
        self.state.lock().unwrap().events.clone()
    }
}
pub(crate) struct WorkerGuard(pub Option<Arc<Probe>>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if let Some(p) = &self.0 {
            p.record("worker_quiescent", 0, 0, 0);
            p.record("detach", 0, 0, 0);
        }
    }
}
pub(crate) struct RuntimeTarget<T> {
    pub target: Arc<T>,
    pub probe: Option<Arc<Probe>>,
}
impl<T> Deref for RuntimeTarget<T> {
    type Target = Arc<T>;
    fn deref(&self) -> &Self::Target {
        &self.target
    }
}
impl<T> Drop for RuntimeTarget<T> {
    fn drop(&mut self) {
        if let Some(p) = &self.probe {
            p.domain_event(
                "runtime_refs",
                0,
                Arc::as_ptr(&self.target) as usize,
                Arc::strong_count(&self.target),
            );
            p.record("runtime_reference_release", 0, 0, 0);
        }
    }
}
