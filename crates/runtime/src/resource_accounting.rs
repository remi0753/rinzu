//! Opt-in observation, separate from resource limits and lifetime decisions.
//! Enable before creating Domains/work; totals persist across observed runs.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

static ENABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Kind {
    Managed,
    ManagedObjects,
    NativeInput,
    Task,
    Future,
    PendingOperation,
    Queue,
    Transport,
    TransportInFlight,
    NativeCalls,
    NativeCopy,
    Registration,
    RuntimeControl,
}
impl Kind {
    pub const ALL: [Self; 13] = [
        Self::Managed,
        Self::ManagedObjects,
        Self::NativeInput,
        Self::Task,
        Self::Future,
        Self::PendingOperation,
        Self::Queue,
        Self::Transport,
        Self::TransportInFlight,
        Self::NativeCalls,
        Self::NativeCopy,
        Self::Registration,
        Self::RuntimeControl,
    ];
    pub const fn id(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::ManagedObjects => "managed_objects",
            Self::NativeInput => "native_input",
            Self::Task => "task",
            Self::Future => "future",
            Self::PendingOperation => "pending_operation",
            Self::Queue => "queue",
            Self::Transport => "transport",
            Self::TransportInFlight => "transport_in_flight",
            Self::NativeCalls => "native_calls",
            Self::NativeCopy => "native_copy",
            Self::Registration => "registration",
            Self::RuntimeControl => "runtime_control",
        }
    }
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Managed | Self::NativeInput | Self::Transport | Self::NativeCopy => "bytes",
            _ => "count",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counter {
    pub live: u64,
    pub allocated: u64,
    pub released: u64,
    pub peak: u64,
}
static COUNTERS: OnceLock<Mutex<[Counter; Kind::ALL.len()]>> = OnceLock::new();
fn counters() -> &'static Mutex<[Counter; Kind::ALL.len()]> {
    COUNTERS.get_or_init(|| Mutex::new([Counter::default(); Kind::ALL.len()]))
}
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}
fn fatal(code: i32) -> ! {
    // SAFETY: observer invariants/OOM follow the runtime's non-unwinding fatal boundary.
    unsafe { libc::_exit(code) }
}
pub fn acquire(kind: Kind, amount: u64) -> bool {
    if !enabled() {
        return false;
    }
    acquire_recorded(kind, amount);
    true
}
pub(crate) fn acquire_recorded(kind: Kind, amount: u64) {
    let mut counters = counters().lock().unwrap_or_else(|_| fatal(86));
    let counter = &mut counters[kind as usize];
    counter.live = counter
        .live
        .checked_add(amount)
        .unwrap_or_else(|| fatal(71));
    counter.allocated = counter
        .allocated
        .checked_add(amount)
        .unwrap_or_else(|| fatal(71));
    counter.peak = counter.peak.max(counter.live);
}
/// Release only amounts whose acquire returned true, even after reporting stops.
pub fn release(kind: Kind, amount: u64) {
    let mut counters = counters().lock().unwrap_or_else(|_| fatal(86));
    let counter = &mut counters[kind as usize];
    counter.live = counter
        .live
        .checked_sub(amount)
        .unwrap_or_else(|| fatal(86));
    counter.released = counter
        .released
        .checked_add(amount)
        .unwrap_or_else(|| fatal(71));
}
pub fn record_copy(amount: u64) {
    if !enabled() {
        return;
    }
    let mut counters = counters().lock().unwrap_or_else(|_| fatal(86));
    let counter = &mut counters[Kind::NativeCopy as usize];
    counter.allocated = counter
        .allocated
        .checked_add(amount)
        .unwrap_or_else(|| fatal(71));
}
pub fn counter(kind: Kind) -> Counter {
    counters().lock().unwrap_or_else(|_| fatal(86))[kind as usize]
}
pub fn counter_snapshot() -> [Counter; Kind::ALL.len()] {
    *counters().lock().unwrap_or_else(|_| fatal(86))
}

#[derive(Debug)]
pub struct Allocation {
    kind: Kind,
    amount: u64,
    active: bool,
}
impl Allocation {
    pub fn new(kind: Kind, amount: u64) -> Self {
        Self {
            kind,
            amount,
            active: acquire(kind, amount),
        }
    }
    pub fn release(&mut self) {
        if self.active {
            release(self.kind, self.amount);
            self.active = false;
        }
    }
}
impl Drop for Allocation {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Debug, Default)]
pub struct FutureRecords(std::collections::BTreeSet<u64>);
impl FutureRecords {
    pub fn insert(&mut self, identity: u64) {
        if acquire(Kind::Future, 1) && !self.0.insert(identity) {
            fatal(86);
        }
    }
    pub fn remove(&mut self, identity: u64) {
        if self.0.remove(&identity) {
            release(Kind::Future, 1);
        }
    }
}
impl Drop for FutureRecords {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            release(Kind::Future, self.0.len() as u64);
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Category {
    pub id: &'static str,
    pub scope: &'static str,
    pub unit: &'static str,
    pub status: &'static str,
    pub live: Option<u64>,
    pub allocated: Option<u64>,
    pub released: Option<u64>,
    pub peak: Option<u64>,
    pub retention_reason: &'static str,
    pub subset_of: Option<&'static str>,
    pub attribution: Attribution,
}
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Attribution {
    pub source_file: Option<u64>,
    pub symbol: Option<String>,
    pub start_byte: Option<u64>,
    pub end_byte: Option<u64>,
    pub precision: &'static str,
}
impl Attribution {
    pub fn unknown() -> Self {
        Self {
            precision: "unknown",
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ReturnSite {
    fallback: bool,
    file: u64,
    start: u32,
    end: u32,
    symbol: String,
}
static RETURN_SITES: OnceLock<Mutex<BTreeMap<ReturnSite, u64>>> = OnceLock::new();
pub fn record_return_site(
    fallback: bool,
    amount: usize,
    source: Option<crate::failure::SourcePosition>,
    symbol: &str,
) {
    if !enabled() {
        return;
    }
    let Some(source) = source else {
        return;
    };
    let mut sites = RETURN_SITES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|_| fatal(86));
    let total = sites
        .entry(ReturnSite {
            fallback,
            file: source.file,
            start: source.start_byte,
            end: source.end_byte,
            symbol: symbol.into(),
        })
        .or_default();
    *total = total
        .checked_add(amount as u64)
        .unwrap_or_else(|| fatal(71));
}
impl Category {
    pub fn unknown(
        id: &'static str,
        scope: &'static str,
        unit: &'static str,
        reason: &'static str,
    ) -> Self {
        Self {
            id,
            scope,
            unit,
            status: "unknown",
            live: None,
            allocated: None,
            released: None,
            peak: None,
            retention_reason: reason,
            subset_of: None,
            attribution: Attribution::unknown(),
        }
    }
    pub fn retained(
        id: &'static str,
        unit: &'static str,
        amount: usize,
        reason: &'static str,
    ) -> Self {
        Self {
            id,
            scope: "process-runtime",
            unit,
            status: "measured",
            live: Some(amount as u64),
            allocated: Some(amount as u64),
            released: Some(0),
            peak: Some(amount as u64),
            retention_reason: reason,
            subset_of: None,
            attribution: Attribution::unknown(),
        }
    }
    pub fn current(
        id: &'static str,
        unit: &'static str,
        amount: usize,
        reason: &'static str,
    ) -> Self {
        Self {
            allocated: None,
            released: None,
            peak: None,
            ..Self::retained(id, unit, amount, reason)
        }
    }
}

pub fn snapshot() -> Vec<Category> {
    let totals = counter_snapshot();
    let mut rows: Vec<_> = Kind::ALL.into_iter().map(|kind| {
        let counter = totals[kind as usize];
        let mut row = Category {id:kind.id(), scope:"process-runtime", unit:kind.unit(), status:"measured",
            live:Some(counter.live), allocated:Some(counter.allocated), released:Some(counter.released),
            peak:Some(counter.peak), retention_reason:match kind {
                Kind::Managed | Kind::ManagedObjects => "Observed Domain semispace use; roots until GC/Domain drop; excludes capacity and GC scratch",
                Kind::NativeInput => "Temporary native receive copy; released on success and UTF-8 failure",
                Kind::Future => "Future identity/result registry until runtime run is dropped",
                Kind::NativeCopy => "Cumulative source native conversion copies; no SDK allocation inference",
                _ => "Observed runtime credits or call objects until physical settlement",
            }, subset_of:None, attribution:Attribution::unknown()};
        if matches!(kind, Kind::NativeCopy) {row.live=None; row.released=None; row.peak=None;}
        row
    }).collect();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let snapshot = crate::callback_return::snapshot();
        let returns = snapshot.process;
        let scopes = snapshot.scopes;
        let scoped = Category {
            id: "callback_scoped",
            scope: "process-runtime",
            unit: "bytes",
            status: "measured",
            live: Some(scopes.live_bytes as u64),
            allocated: Some(scopes.allocated_bytes as u64),
            released: Some(scopes.released_bytes as u64),
            peak: Some(scopes.peak_bytes as u64),
            retention_reason: "Actual return owner until closed proof + physical readers/receipts settle; promotion is transfer, not free",
            subset_of: None,
            attribution: Attribution::unknown(),
        };
        let mut promoted = Category::current(
            "callback_promoted",
            "bytes",
            0,
            "Cumulative missing-proof/receipt transfer into process arena; included in normal/fallback retention; no extra copy",
        );
        promoted.live = None;
        promoted.allocated = Some(scopes.promoted_bytes as u64);
        let mut copies = Category::current(
            "callback_return_copy",
            "bytes",
            0,
            "Cumulative explicit memcpy prefix of process/scoped return factories; excludes initialized padding and GC copying",
        );
        copies.live = None;
        copies.allocated = Some(
            (snapshot.process_copied_bytes as u64)
                .checked_add(scopes.copied_bytes as u64)
                .unwrap_or_else(|| fatal(71)),
        );
        rows.extend([scoped, promoted, copies,
            Category::current("callback_scoped_metadata", "bytes", scopes.metadata_bytes,
                "Current scope + node metadata until consumption/promotion; promoted nodes are in callback_return_metadata"),
            Category::current("callback_scoped_owners", "count", scopes.active_scopes,
                "Actual prepared/exposed owners until physical consumption or conservative promotion")]);
        rows.extend([
            Category::retained(
                "callback_normal",
                "bytes",
                returns.normal_bytes,
                "Immutable callback payload until process exit; includes NUL/minimum byte",
            ),
            Category::retained(
                "callback_fallback",
                "bytes",
                returns.fallback_bytes,
                "Immutable fallback payload until process exit; separate from normal",
            ),
            Category::retained(
                "callback_return_metadata",
                "bytes",
                returns.metadata_bytes,
                "Arena node metadata until process exit; excludes allocator overhead",
            ),
        ]);
        let pool = crate::callback_entry::stats();
        rows.extend([
            Category::current(
                "callback_control",
                "bytes",
                pool.control_bytes,
                "Current full controls plus permanent RW reservation; proof retires only full controls",
            ),
            Category::retained(
                "callback_tombstone",
                "bytes",
                pool.tombstone_bytes,
                "Unknown or unretired full control for late invocation; NULL slots reported separately",
            ),
            Category::retained(
                "callback_rx",
                "bytes",
                pool.virtual_bytes,
                "Process lifetime RX/RW virtual reservation; separate from RSS",
            ),
            Category::retained(
                "callback_block_metadata",
                "bytes",
                pool.block_metadata_bytes,
                "Callback block metadata until process exit",
            ),
            Category::retained(
                "callback_entries",
                "count",
                pool.entries,
                "Published callback entries including closed entries",
            ),
            Category::current(
                "callback_in_flight",
                "count",
                pool.in_flight,
                "Admitted callback invocations until physical return",
            ),
            Category::retained(
                "callback_retired_entries", "count", pool.retired_entries,
                "Permanent NULL entries after proof/physical/once retirement; addresses never reused",
            ),
            Category::retained(
                "callback_retired_slots", "bytes", pool.retired_slot_bytes,
                "NULL slots are a subset of the existing RW reservation, not new allocation",
            ),
        ]);
        rows.iter_mut()
            .find(|row| row.id == "callback_tombstone")
            .unwrap()
            .subset_of = Some("callback_control");
        let control = rows
            .iter_mut()
            .find(|row| row.id == "callback_control")
            .unwrap();
        control.allocated = Some(
            pool.control_bytes
                .checked_add(pool.released_control_bytes)
                .unwrap_or_else(|| fatal(71)) as u64,
        );
        control.released = Some(pool.released_control_bytes as u64);
        rows.iter_mut()
            .find(|row| row.id == "callback_retired_entries")
            .unwrap()
            .subset_of = Some("callback_entries");
        rows.iter_mut()
            .find(|row| row.id == "callback_retired_slots")
            .unwrap()
            .subset_of = Some("callback_control");
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    for (id, unit) in [
        ("callback_normal", "bytes"),
        ("callback_fallback", "bytes"),
        ("callback_return_metadata", "bytes"),
        ("callback_scoped", "bytes"),
        ("callback_promoted", "bytes"),
        ("callback_return_copy", "bytes"),
        ("callback_scoped_metadata", "bytes"),
        ("callback_scoped_owners", "count"),
        ("callback_control", "bytes"),
        ("callback_tombstone", "bytes"),
        ("callback_rx", "bytes"),
        ("callback_block_metadata", "bytes"),
        ("callback_entries", "count"),
        ("callback_retired_entries", "count"),
        ("callback_retired_slots", "bytes"),
        ("callback_in_flight", "count"),
    ] {
        let mut row = Category::unknown(
            id,
            "process-runtime",
            unit,
            "Callback arena/pool is unsupported on this target",
        );
        row.status = "unsupported";
        rows.push(row);
    }
    let userdata = crate::data::userdata_process_stats();
    rows.push(Category::retained(
        "userdata_identity",
        "bytes",
        userdata.token_bytes,
        "Process token identity table; excludes hash table capacity/allocator overhead",
    ));
    rows.push(Category::retained(
        "userdata_tokens",
        "count",
        userdata.tokens,
        "Process token identities; the bytes row describes storage separately",
    ));
    let mut closed = Category::retained(
        "userdata_closed",
        "bytes",
        userdata
            .closed
            .checked_mul(
                userdata
                    .token_bytes
                    .checked_div(userdata.tokens)
                    .unwrap_or(0),
            )
            .unwrap_or_else(|| fatal(71)),
        "Closed token identities retained for process lifetime",
    );
    closed.subset_of = Some("userdata_identity");
    rows.push(closed);
    let mut closed_count = Category::retained(
        "userdata_closed_tokens",
        "count",
        userdata.closed,
        "Closed token count; subset of userdata_tokens",
    );
    closed_count.subset_of = Some("userdata_tokens");
    rows.push(closed_count);
    rows.extend([
        Category::unknown(
            "foreign_queue",
            "run",
            "bytes",
            "Per-queue snapshot is published at run end; no process-wide queue registry",
        ),
        Category::unknown(
            "foreign_records",
            "run",
            "count",
            "Per-queue snapshot is published at run end",
        ),
        Category::unknown(
            "jit_rx",
            "run",
            "bytes",
            "Code size is not mapping size; mapping observation required",
        ),
        Category::unknown(
            "sdk_memory",
            "sdk",
            "bytes",
            "SDK has not reported allocator-owned memory",
        ),
    ]);
    if let Some(sites) = RETURN_SITES.get() {
        for (site, amount) in sites.lock().unwrap_or_else(|_| fatal(86)).iter() {
            let mut row = Category::retained(
                if site.fallback {
                    "callback_fallback_site"
                } else {
                    "callback_normal_site"
                },
                "bytes",
                usize::try_from(*amount).unwrap_or_else(|_| fatal(71)),
                "Known callback site; subset of process payload retention",
            );
            row.subset_of = Some(if site.fallback {
                "callback_fallback"
            } else {
                "callback_normal"
            });
            row.attribution = Attribution {
                source_file: Some(site.file),
                symbol: Some(site.symbol.clone()),
                start_byte: Some(site.start as u64),
                end_byte: Some(site.end as u64),
                precision: "site",
            };
            rows.push(row);
        }
    }
    rows.extend(os_snapshot());
    rows
}

pub fn os_snapshot() -> Vec<Category> {
    let mut rows: Vec<_> = ["rss", "virtual", "footprint"].into_iter().map(|id|
        Category::unknown(id,"process-os","bytes","Independent OS measurement; includes sampler and SDK memory; not a sum of runtime categories")
    ).collect();
    #[cfg(target_os = "macos")]
    {
        // SAFETY: these are initialized, fixed-layout libc output records. Sizes
        // and flavor match the installed macOS SDK's libproc/sys/resource ABI.
        let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = i32::try_from(std::mem::size_of_val(&info)).unwrap();
        // SAFETY: writable buffer covers exactly size bytes for this process.
        let returned = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                std::ptr::from_mut(&mut info).cast(),
                size,
            )
        };
        if returned == size {
            rows[0].status = "measured";
            rows[0].live = Some(info.pti_resident_size);
            rows[1].status = "measured";
            rows[1].live = Some(info.pti_virtual_size);
        }
        // SAFETY: rusage_info_v0 exactly matches RUSAGE_INFO_V0's output extent.
        let mut usage: libc::rusage_info_v0 = unsafe { std::mem::zeroed() };
        // SAFETY: libproc takes rusage_info_t* as the address of the output struct,
        // as specified in libproc.h; it does not dereference a pointer slot first.
        if unsafe {
            libc::proc_pid_rusage(
                libc::getpid(),
                libc::RUSAGE_INFO_V0,
                std::ptr::from_mut(&mut usage).cast(),
            )
        } == 0
        {
            rows[2].status = "measured";
            rows[2].live = Some(usage.ri_phys_footprint);
        }
    }
    #[cfg(not(target_os = "macos"))]
    for row in &mut rows {
        row.status = "unsupported";
    }
    rows
}
