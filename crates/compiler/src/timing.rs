//! Opt-in compilation timings. Program output and normal diagnostics are unchanged.
use std::cell::Cell;
use std::io::Write;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

thread_local! {
    static PARENT: Cell<Option<u64>> = const { Cell::new(None) };
}

pub struct Span {
    active: Option<ActiveSpan>,
    owner_only: PhantomData<Rc<()>>,
}

struct ActiveSpan {
    phase: &'static str,
    detail: Option<String>,
    id: u64,
    parent: Option<u64>,
    started: Instant,
    duration: Option<std::time::Duration>,
    counters: std::collections::BTreeMap<String, u64>,
}

/// Set `RINZU_COMPILE_TIMINGS=1` to emit nested JSON records to stderr.
pub fn span(phase: &'static str) -> Span {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let active = if *ENABLED.get_or_init(|| std::env::var_os("RINZU_COMPILE_TIMINGS").is_some()) {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Some(ActiveSpan {
            phase,
            detail: None,
            id,
            parent: PARENT.with(|parent| parent.replace(Some(id))),
            started: Instant::now(),
            duration: None,
            counters: std::collections::BTreeMap::new(),
        })
    } else {
        None
    };
    Span {
        active,
        owner_only: PhantomData,
    }
}

impl Span {
    pub fn set_counter(&mut self, name: &str, value: u64) {
        if let Some(active) = &mut self.active {
            active.counters.insert(name.to_owned(), value);
        }
    }
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Record a measured foreign-code subinterval under the current parent.
    pub fn finish_with_duration(mut self, duration: std::time::Duration) {
        if let Some(active) = &mut self.active {
            active.duration = Some(duration);
        }
    }

    pub fn detail(mut self, detail: &str) -> Self {
        if let Some(active) = &mut self.active {
            active.detail = Some(detail.to_owned());
        }
        self
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(active) = &self.active {
            let elapsed_ns = active
                .duration
                .unwrap_or_else(|| active.started.elapsed())
                .as_nanos();
            PARENT.with(|parent| parent.set(active.parent));
            let record = serde_json::json!({
                "phase": active.phase, "detail": active.detail,
                "id": active.id, "parent": active.parent, "elapsed_ns": elapsed_ns,
                "thread": format!("{:?}", std::thread::current().id()),
                "counters": active.counters,
            });
            let _ = writeln!(std::io::stderr().lock(), "RINZU_COMPILE_TIMING={record}");
        }
    }
}
