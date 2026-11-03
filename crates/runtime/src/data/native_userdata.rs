//! Actual-argument userdata identities, independent of moving heap addresses and
//! callback contexts. Tokens are never-reused numbers, looked up only in a live call.
use super::*;
use crate::value::ValueType;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread::ThreadId;

fn fatal() -> ! {
    // SAFETY: malformed interop identity never allocates, executes managed code
    // or unwinds across a native frame.
    crate::native_fatal::invariant()
}
fn oom() -> ! {
    // SAFETY: known allocation/resource failure has a distinct fatal path.
    crate::native_fatal::resource()
}
fn require<T, E>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|_| fatal())
}
fn identity() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        value.checked_add(1)
    })
    .unwrap_or_else(|_| oom())
}
/// Private bridge for compiled runtime images embedded in this same process.
/// Sharing this issuer prevents a stale token from matching a different image.
pub extern "C" fn native_context_identity() -> u64 {
    identity()
}
static LIVE_TOKENS: AtomicUsize = AtomicUsize::new(0);
fn token_acquired() {
    LIVE_TOKENS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count
                .checked_add(1)
                .filter(|count| *count <= usize::MAX / std::mem::size_of::<Argument>())
        })
        .unwrap_or_else(|_| oom());
}
fn token_released() {
    LIVE_TOKENS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
            count.checked_sub(1)
        })
        .unwrap_or_else(|_| fatal());
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NativeUserDataToken(usize);
impl NativeUserDataToken {
    pub const fn pointer(self) -> *const u8 {
        self.0 as *const u8
    }
    pub const fn bits(self) -> u64 {
        self.0 as u64
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NativeUserDataScope(u64);
impl NativeUserDataScope {
    pub const fn bits(self) -> u64 {
        self.0
    }
}
/// Private admission identity; resolution also checks its live call gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeUserDataInvocation {
    scope: NativeUserDataScope,
    identity: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeUserDataError {
    Null,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeUserDataEvidence {
    pub scopes: usize,
    pub tokens: usize,
    pub roots: usize,
    pub in_flight: usize,
    pub created: usize,
    pub released: usize,
}
impl NativeUserDataEvidence {
    pub fn is_baseline(self) -> bool {
        self.scopes == 0
            && self.tokens == 0
            && self.roots == 0
            && self.in_flight == 0
            && self.created == self.released
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativeUserDataProcessStats {
    pub tokens: usize,
    pub closed: usize,
    pub token_bytes: usize,
    pub registry_capacity: usize,
}
pub fn userdata_process_stats() -> NativeUserDataProcessStats {
    let tokens = LIVE_TOKENS.load(Ordering::Relaxed);
    NativeUserDataProcessStats {
        tokens,
        closed: 0,
        token_bytes: tokens
            .checked_mul(std::mem::size_of::<Argument>())
            .unwrap_or_else(|| oom()),
        // There is no process lookup table or retained capacity.
        registry_capacity: 0,
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Preparing,
    Open,
    Settling,
}
struct Argument {
    ty: usize,
    value: ManagedHandle,
    root: RootLease,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum ResultHold {
    Pending(usize),
    Read(usize),
}
struct Call {
    heap: usize,
    data: usize,
    roots: Option<usize>,
    phase: Phase,
    arguments: HashMap<usize, Argument>,
    invocations: HashSet<u64>,
    result: Option<ResultHold>,
}
pub struct NativeUserDataRegistry {
    domain: u64,
    owner: ThreadId,
    calls: HashMap<NativeUserDataScope, Call>,
    created: usize,
    released: usize,
    _owner: PhantomData<Rc<()>>,
}
impl NativeUserDataRegistry {
    fn call_owner(
        call: &Call,
        heap: &DomainHeap,
        data: Option<&ManagedData>,
        roots: Option<&ManagedRoots>,
    ) {
        if call.heap != std::ptr::from_ref(heap) as usize
            || data.is_some_and(|data| call.data != std::ptr::from_ref(data) as usize)
            || roots.is_some_and(|roots| {
                call.roots
                    .is_some_and(|owner| owner != std::ptr::from_ref(roots) as usize)
            })
        {
            fatal();
        }
    }
    pub fn new(heap: &DomainHeap) -> Self {
        if heap.owner_thread() != std::thread::current().id() {
            fatal();
        }
        Self {
            domain: heap.domain_id(),
            owner: heap.owner_thread(),
            calls: HashMap::new(),
            created: 0,
            released: 0,
            _owner: PhantomData,
        }
    }
    fn check(&self, heap: &DomainHeap) {
        if self.owner != std::thread::current().id()
            || heap.owner_thread() != self.owner
            || heap.domain_id() != self.domain
        {
            fatal();
        }
    }
    fn mapping(data: &ManagedData, heap: &DomainHeap, ty: usize) {
        require(data.require_domain(heap));
        let schema = require(data.value_schema());
        // A context is retained in its Domain, never compared or transferred.
        // Its fields may contain mutable classes, closures and native references.
        if !matches!(
            require(schema.ty(ty)),
            ValueType::Record { .. } | ValueType::State { .. } | ValueType::Enum { .. }
        ) {
            fatal();
        }
    }
    fn value(data: &ManagedData, heap: &DomainHeap, ty: usize, value: ManagedHandle) {
        Self::mapping(data, heap, ty);
        let (root, kind) = data.roots.get(&value).unwrap_or_else(|| fatal());
        if *kind != DataKind::Composite(ty) || require(heap.root_target(*root)).is_none() {
            fatal();
        }
    }
    pub fn evidence(&self) -> NativeUserDataEvidence {
        let tokens = self.calls.values().map(|call| call.arguments.len()).sum();
        NativeUserDataEvidence {
            scopes: self.calls.len(),
            tokens,
            roots: tokens,
            in_flight: self.calls.values().map(|call| call.invocations.len()).sum(),
            created: self.created,
            released: self.released,
        }
    }
    pub fn finish(&self, heap: &DomainHeap) -> NativeUserDataEvidence {
        self.check(heap);
        let evidence = self.evidence();
        if !evidence.is_baseline() {
            fatal();
        }
        evidence
    }
    pub fn enter_scope(&mut self, heap: &DomainHeap, data: &ManagedData) -> NativeUserDataScope {
        self.check(heap);
        require(data.require_domain(heap));
        if self.calls.try_reserve(1).is_err() {
            oom();
        }
        let scope = NativeUserDataScope(identity());
        self.calls.insert(
            scope,
            Call {
                heap: std::ptr::from_ref(heap) as usize,
                data: std::ptr::from_ref(data) as usize,
                roots: None,
                phase: Phase::Preparing,
                arguments: HashMap::new(),
                invocations: HashSet::new(),
                result: None,
            },
        );
        scope
    }
    /// Pin each actual argument independently, even when two values are equal.
    pub fn prepare(
        &mut self,
        heap: &DomainHeap,
        data: &ManagedData,
        roots: &mut ManagedRoots,
        scope: NativeUserDataScope,
        ty: usize,
        value: ManagedHandle,
    ) -> NativeUserDataToken {
        self.check(heap);
        let call = self.calls.get_mut(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, Some(data), Some(roots));
        Self::value(data, heap, ty, value);
        if call.phase != Phase::Preparing {
            fatal();
        }
        if call.arguments.try_reserve(1).is_err() {
            oom();
        }
        let created = self.created.checked_add(1).unwrap_or_else(|| oom());
        let pointer = usize::try_from(identity()).unwrap_or_else(|_| oom());
        let root = roots.pin(value).unwrap_or_else(|_| oom());
        call.roots = Some(std::ptr::from_ref(roots) as usize);
        if call
            .arguments
            .insert(pointer, Argument { ty, value, root })
            .is_some()
        {
            fatal();
        }
        token_acquired();
        self.created = created;
        NativeUserDataToken(pointer)
    }
    /// Reserve an ordinary-record result before entry. Its argument mappings
    /// survive physical return and callback settlement until finish_result.
    pub fn expect_result(
        &mut self,
        heap: &DomainHeap,
        data: &ManagedData,
        scope: NativeUserDataScope,
        ty: usize,
    ) {
        self.check(heap);
        Self::mapping(data, heap, ty);
        let call = self.calls.get_mut(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, Some(data), None);
        if call.phase != Phase::Preparing || call.result.is_some() {
            fatal();
        }
        call.result = Some(ResultHold::Pending(ty));
    }
    /// No callback admission is manufactured for a physical native return.
    /// Even NULL requires the exact result contract and a fully settled call.
    pub fn receive_result(
        &mut self,
        heap: &DomainHeap,
        data: &ManagedData,
        scope: NativeUserDataScope,
        ty: usize,
        pointer: *const u8,
    ) -> Result<ManagedHandle, NativeUserDataError> {
        self.check(heap);
        let call = self.calls.get_mut(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, Some(data), None);
        if call.phase != Phase::Settling
            || !call.invocations.is_empty()
            || call.result != Some(ResultHold::Pending(ty))
        {
            fatal();
        }
        Self::mapping(data, heap, ty);
        call.result = Some(ResultHold::Read(ty));
        if pointer.is_null() {
            return Err(NativeUserDataError::Null);
        }
        let argument = call
            .arguments
            .get(&(pointer as usize))
            .unwrap_or_else(|| fatal());
        if argument.ty != ty {
            fatal();
        }
        Self::value(data, heap, ty, argument.value);
        Ok(argument.value)
    }
    /// Release after a read (including NULL), or discard a cancelled result
    /// without conversion. Physical return and callback settlement are required.
    pub fn finish_result(
        &mut self,
        heap: &DomainHeap,
        roots: &mut ManagedRoots,
        scope: NativeUserDataScope,
    ) {
        self.check(heap);
        let call = self.calls.get(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, None, Some(roots));
        if call.phase != Phase::Settling || !call.invocations.is_empty() || call.result.is_none() {
            fatal();
        }
        self.release(roots, scope);
    }
    pub fn open(&mut self, heap: &DomainHeap, scope: NativeUserDataScope) {
        self.check(heap);
        let call = self.calls.get_mut(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, None, None);
        if call.phase != Phase::Preparing {
            fatal();
        }
        call.phase = Phase::Open;
    }
    pub fn admit(
        &mut self,
        heap: &DomainHeap,
        scope: NativeUserDataScope,
    ) -> NativeUserDataInvocation {
        self.check(heap);
        let call = self.calls.get_mut(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, None, None);
        if call.phase != Phase::Open {
            fatal();
        }
        if call.invocations.try_reserve(1).is_err() {
            oom();
        }
        let id = identity();
        call.invocations.insert(id);
        NativeUserDataInvocation {
            scope,
            identity: id,
        }
    }
    /// Validate admission before NULL handling, then registered pointer identity
    /// before consulting metadata. Settling permits only previously admitted work.
    pub fn resolve(
        &self,
        heap: &DomainHeap,
        data: &ManagedData,
        invocation: NativeUserDataInvocation,
        ty: usize,
        pointer: *const u8,
    ) -> Result<ManagedHandle, NativeUserDataError> {
        self.check(heap);
        let call = self.calls.get(&invocation.scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, Some(data), None);
        if !call.invocations.contains(&invocation.identity) || call.phase == Phase::Preparing {
            fatal();
        }
        Self::mapping(data, heap, ty);
        if pointer.is_null() {
            return Err(NativeUserDataError::Null);
        }
        let address = pointer as usize;
        let argument = call.arguments.get(&address).unwrap_or_else(|| fatal());
        if argument.ty != ty {
            fatal();
        }
        Self::value(data, heap, ty, argument.value);
        Ok(argument.value)
    }
    fn release(&mut self, roots: &mut ManagedRoots, scope: NativeUserDataScope) {
        let call = self.calls.remove(&scope).unwrap_or_else(|| fatal());
        if !call.invocations.is_empty() {
            fatal();
        }
        for argument in call.arguments.into_values() {
            if roots.leases.get(&argument.root) != Some(&argument.value) {
                fatal();
            }
            require(roots.unpin(argument.root));
            token_released();
            self.released = self.released.checked_add(1).unwrap_or_else(|| oom());
        }
    }
    pub fn discard_prepared(
        &mut self,
        heap: &DomainHeap,
        roots: &mut ManagedRoots,
        scope: NativeUserDataScope,
    ) {
        self.check(heap);
        Self::call_owner(
            self.calls.get(&scope).unwrap_or_else(|| fatal()),
            heap,
            None,
            Some(roots),
        );
        if self.calls.get(&scope).unwrap_or_else(|| fatal()).phase != Phase::Preparing {
            fatal();
        }
        self.release(roots, scope);
    }
    /// Called only after the actual native return; no synthetic return on cancel.
    pub fn returned(
        &mut self,
        heap: &DomainHeap,
        roots: &mut ManagedRoots,
        scope: NativeUserDataScope,
    ) {
        self.check(heap);
        let call = self.calls.get_mut(&scope).unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, None, Some(roots));
        if call.phase != Phase::Open {
            fatal();
        }
        call.phase = Phase::Settling;
        if call.invocations.is_empty() && call.result.is_none() {
            self.release(roots, scope);
        }
    }
    pub fn settle(
        &mut self,
        heap: &DomainHeap,
        roots: &mut ManagedRoots,
        invocation: NativeUserDataInvocation,
    ) {
        self.check(heap);
        let call = self
            .calls
            .get_mut(&invocation.scope)
            .unwrap_or_else(|| fatal());
        Self::call_owner(call, heap, None, Some(roots));
        if !call.invocations.remove(&invocation.identity) {
            fatal();
        }
        if call.phase == Phase::Settling && call.invocations.is_empty() && call.result.is_none() {
            self.release(roots, invocation.scope);
        }
    }
}
impl Drop for NativeUserDataRegistry {
    fn drop(&mut self) {
        if !self.calls.is_empty() {
            fatal();
        }
    }
}
