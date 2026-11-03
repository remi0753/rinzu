//! Source-level async execution over the production scheduler and services.
//!
//! This is the baseline continuation engine.  It deliberately consumes typed
//! IR rather than package names: SDK package metadata supplies the native
//! operation table, and the engine owns suspension, resumption, and Worker
//! Domain admission.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use rinzu_abi::{ABI_VERSION, NativeInvocationContextV1};
use rinzu_compiler::{
    BinaryOperator, BlockId, ConstantValue, IrFunction, IrModule, OperationKind, Terminator,
    UnaryOperator, ValueId,
};
use rinzu_runtime::child_scope::ChildScope;
use rinzu_runtime::data::ManagedHandle;
use rinzu_runtime::failure_transport::{failure_descriptor, failure_value, materialize_failure};
use rinzu_runtime::reactor::{
    AsyncReactor, FileReadResult, ReactorCompletion, ReactorError, ReactorOperation,
};
use rinzu_runtime::scalar::{Arithmetic, Comparison, ScalarKind, ScalarValue};
use rinzu_runtime::scheduler::{OperationId, Resolution, TaskId, TaskOutcome};
use rinzu_runtime::spawn::{
    AdmittedSpawn, OriginDomainId, SpawnAdmissionController, SpawnAdmissionError,
    SpawnAdmissionSnapshot,
};
use rinzu_runtime::standalone::{
    ManagedTask, OperationMailbox, RunStatus, StandaloneError, StandaloneOptions,
    StandaloneRuntime, TaskAction, TaskEvent,
};
use rinzu_runtime::transport::{
    TransportBuffer, TransportError, typed::Descriptor as TypedDescriptor,
};
use rinzu_runtime::worker::{
    CpuWorkerPool, WorkerCompletion, WorkerDomain, WorkerResult, WorkerWaitError,
};

mod data;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod dispatcher;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native;
pub use data::HeapEvidence as SourceHeapEvidence;
use data::{OwnerFrame, OwnerValues};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use native::{NativeBindings, NativeEngine};

use crate::LoadedNativePackages;
use crate::source_failure::{FailureRoutes, ManagedFailure};
use rinzu_runtime::failure::{Diagnostic, ErrorKind, Failure, FailureCause, SourcePosition};

#[cfg(all(
    feature = "test-dispatcher-probe",
    target_os = "macos",
    target_arch = "aarch64"
))]
thread_local! {static FOREIGN_RETURN_PROBE: Cell<Option<usize>> = const {Cell::new(None)};}
#[cfg(all(
    feature = "test-dispatcher-probe",
    target_os = "macos",
    target_arch = "aarch64"
))]
pub fn configure_foreign_return_probe(count: usize) {
    FOREIGN_RETURN_PROBE.with(|probe| probe.set(Some(count)));
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeAsyncOperation {
    FileReadSize { path_argument: usize },
}

#[derive(Debug)]
pub struct SourceAsyncOptions {
    pub capture_stdout: bool,
    pub capture_error_trace: bool,
    /// Conformance mode: force moving GC at every typed root publication.
    pub gc_stress: bool,
    pub package_root: PathBuf,
    pub native_async: BTreeMap<String, NativeAsyncOperation>,
    /// Physical completion wait interval. Expiry polls again; it does not end
    /// a running native helper or release its roots/admission.
    pub completion_timeout: Duration,
    pub cancel_first_spawn: bool,
    pub cancel_after_polls: Option<usize>,
    pub shutdown_after_polls: Option<usize>,
    pub cancel_worker_after_polls: Option<usize>,
    /// Private conformance request on a Worker's own managed polls.
    pub shutdown_worker_after_polls: Option<usize>,
    pub record_transport_evidence: bool,
    /// Private conformance profile. Ordinary runs grow with available memory.
    pub main_task_limit: Option<usize>,
    pub native_packages: Option<LoadedNativePackages>,
    pub source_packages: Option<crate::source_package::LoadedSourcePackages>,
}

impl Default for SourceAsyncOptions {
    fn default() -> Self {
        Self {
            capture_stdout: false,
            capture_error_trace: false,
            gc_stress: false,
            package_root: PathBuf::from("."),
            native_async: BTreeMap::new(),
            completion_timeout: Duration::from_secs(2),
            cancel_first_spawn: false,
            cancel_after_polls: None,
            shutdown_after_polls: None,
            cancel_worker_after_polls: None,
            shutdown_worker_after_polls: None,
            record_transport_evidence: false,
            main_task_limit: None,
            native_packages: None,
            source_packages: None,
        }
    }
}

impl SourceAsyncOptions {
    pub fn from_sdk_packages(
        package_root: impl Into<PathBuf>,
        packages: &[rinzu_compiler::ResolvedSdkPackage],
    ) -> Result<Self> {
        let mut native_async = BTreeMap::new();
        for package in packages {
            for import in &package.native_imports {
                let Some(service) = import.runtime_service.as_deref() else {
                    continue;
                };
                let operation = match service {
                    "reactor.file-read-size" => {
                        NativeAsyncOperation::FileReadSize { path_argument: 0 }
                    }
                    _ => bail!(
                        "SDK export {} requires unsupported runtime service {service}",
                        import.export_name
                    ),
                };
                if native_async
                    .insert(import.export_name.clone(), operation)
                    .is_some()
                {
                    bail!("duplicate runtime service export {}", import.export_name);
                }
            }
        }
        Ok(Self {
            package_root: package_root.into(),
            native_async,
            ..Self::default()
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceAsyncEvent {
    Submitted {
        future: u64,
        operation: String,
    },
    Suspended {
        scope: u64,
        future: u64,
        operation: String,
        owner_thread: ThreadId,
    },
    FileCompleted {
        future: u64,
        bytes: usize,
    },
    TimerCompleted {
        future: u64,
    },
    WorkerCompleted {
        future: u64,
        domain: u64,
        worker_thread: ThreadId,
    },
    Resumed {
        scope: u64,
        future: u64,
        owner_thread: ThreadId,
    },
    /// Every explicit await observation, including an already ready Future.
    Awaited {
        scope: u64,
        future: u64,
    },
    Cancelled {
        future: u64,
    },
    NativeEntered {
        scope: u64,
        export: String,
    },
    CallbackEntered {
        scope: u64,
        handler: String,
    },
    CallbackLeft {
        scope: u64,
        handler: String,
    },
    NativeLeft {
        scope: u64,
        export: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceAsyncRun {
    /// Requested process status after successful physical settlement. Embedders
    /// observe this value; the runtime never terminates their host process.
    pub exit_status: u8,
    pub future_admissions: Vec<FutureAdmission>,
    pub peak_future_records: usize,
    pub gc_retired_futures: usize,
    pub remaining_future_records: usize,
    pub stdout: Vec<u8>,
    pub result: i64,
    pub failure_details: Option<Failure>,
    pub shutdown_complete: bool,
    pub caught_errors: Vec<Failure>,
    pub source_files: BTreeMap<u64, String>,
    pub last_awaited_result: Option<i64>,
    pub last_awaited_scalar: Option<ScalarValue>,
    pub awaited_scalars: Vec<ScalarValue>,
    /// Exact await order within each task, independent of other tasks' timing.
    pub awaited_tasks: BTreeMap<u64, Vec<(u64, ScalarValue)>>,
    pub transport: SpawnAdmissionSnapshot,
    pub transport_evidence: Vec<rinzu_runtime::transport::TransportCreditEvidence>,
    pub cancelled_children: Vec<u64>,
    /// Scope-close cancellation attempts, including a late physical winner.
    pub scope_cancellation_requests: Vec<u64>,
    /// Cancellation actually requested on Main, independently of child control.
    pub root_cancellation_requested: bool,
    /// Explicit ready/cancel/timeout queries, including a false result.
    pub control_attempts: Vec<u64>,
    pub owner_thread: ThreadId,
    pub events: Vec<SourceAsyncEvent>,
    pub resources_returned: bool,
    pub heap: SourceHeapEvidence,
    pub worker_heaps: Vec<SourceWorkerHeapEvidence>,
    /// Physical CPU-pool returns, including cancellation before body entry.
    pub worker_returns: usize,
    pub native_calls: SourceNativeCallEvidence,
    /// Process-wide snapshot; other concurrent runs can contribute to it.
    pub native_process: crate::NativeProcessEvidence,
}

/// Private proof identity: admission ordinal inside its Origin task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FutureAdmission {
    pub future: u64,
    pub parent_scope: u64,
    pub ordinal: u64,
}

/// Completed ledger transitions and the remaining call-lifetime resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceNativeCallEvidence {
    pub prepared: usize,
    pub returned: usize,
    pub closed: usize,
    pub settled: usize,
    pub read: usize,
    pub released: usize,
    pub abandoned: usize,
    pub callback_entries: usize,
    pub copied_bytes: usize,
    pub live_calls: usize,
    pub live_buffers: usize,
    pub live_callbacks: usize,
    pub callback_returns: CallbackReturnEvidence,
    pub foreign: ForeignInvocationEvidence,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForeignInvocationEvidence {
    pub created: u64,
    pub published: u64,
    pub received: u64,
    pub retired: u64,
    pub live: usize,
    pub bytes: usize,
    pub queued: usize,
    pub wake_pins: usize,
}
impl ForeignInvocationEvidence {
    pub fn is_baseline(self) -> bool {
        self.created == self.published
            && self.created == self.received
            && self.created == self.retired
            && self.live == 0
            && self.bytes == 0
            && self.queued == 0
            && self.wake_pins == 0
    }
}

/// Cumulative native return copies attributed to this run, including released
/// scoped copies. Current process retention is a separate snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallbackReturnEvidence {
    pub normal: usize,
    pub fallback: usize,
    pub bytes: usize,
    pub metadata_bytes: usize,
}
impl CallbackReturnEvidence {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn record(
        &mut self,
        buffer: &rinzu_runtime::callback_return::ProducedBuffer,
        fallback: bool,
    ) {
        let count = if fallback {
            &mut self.fallback
        } else {
            &mut self.normal
        };
        *count = count
            .checked_add(1)
            .unwrap_or_else(|| std::process::abort());
        self.bytes = self
            .bytes
            .checked_add(buffer.storage_bytes())
            .unwrap_or_else(|| std::process::abort());
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(buffer.metadata_bytes())
            .unwrap_or_else(|| std::process::abort());
    }
}

impl SourceNativeCallEvidence {
    pub fn is_baseline(self) -> bool {
        self.live_calls == 0
            && self.live_buffers == 0
            && self.live_callbacks == 0
            && self.foreign.is_baseline()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceWorkerHeapEvidence {
    pub domain: u64,
    pub heap: SourceHeapEvidence,
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Unit,
    Scalar(ScalarValue),
    String(String),
    Bytes(Vec<u8>),
    Handler(String),
    FunctionReference(String),
    Error(Failure),
    Future(u64),
    Managed { ty: usize, handle: ManagedHandle },
}

impl Value {
    fn as_i64(&self) -> Result<i64> {
        match self {
            Self::Unit => Ok(0),
            Self::Scalar(value) => Ok(value.bits() as i64),
            value => bail!("source async result is not scalar: {value:?}"),
        }
    }
    fn int(value: i64) -> Self {
        Self::Scalar(
            ScalarValue::from_integer(ScalarKind::I64, i128::from(value))
                .expect("i64 source value"),
        )
    }
    fn scalar(&self) -> Option<ScalarValue> {
        match self {
            Self::Scalar(value) => Some(*value),
            _ => None,
        }
    }
}

fn resolve_indirect<'a>(
    operation: &'a rinzu_compiler::Operation,
    values: &HashMap<ValueId, Value>,
) -> Result<std::borrow::Cow<'a, rinzu_compiler::Operation>> {
    let OperationKind::IndirectCall {
        callee,
        targets,
        arguments,
        failure,
        error,
        cleanup_chain,
    } = &operation.kind
    else {
        return Ok(std::borrow::Cow::Borrowed(operation));
    };
    let Some(Value::FunctionReference(target)) = values.get(callee) else {
        bail!("indirect callee is not a typed function reference");
    };
    ensure!(
        targets.contains(target),
        "indirect callee is not in its closed target set"
    );
    let mut resolved = operation.clone();
    resolved.kind = if let Some((kind, _)) = crate::function_reference::core_length(target) {
        ensure!(
            arguments.len() == 1,
            "length reference argument count mismatch"
        );
        OperationKind::CoreLength {
            operand: arguments[0],
            kind,
        }
    } else {
        OperationKind::ManagedCall {
            target: target.clone(),
            arguments: arguments.clone(),
            failure: *failure,
            error: error.clone(),
            cleanup_chain: cleanup_chain.clone(),
        }
    };
    Ok(std::borrow::Cow::Owned(resolved))
}

#[derive(Debug)]
enum PendingRequest {
    NativeCompletion {
        completion: rinzu_runtime::native_completion::CompletionPointer,
        transfer: bool,
        source: Option<SourcePosition>,
    },
    FileReadSize {
        path: String,
        source: Option<SourcePosition>,
    },
    Timer {
        milliseconds: i64,
        source: Option<SourcePosition>,
    },
    Spawn {
        target: String,
        prepared: Box<PreparedSpawn>,
        source: Option<SourcePosition>,
    },
}

#[derive(Debug)]
struct PreparedSpawn {
    descriptor: TypedDescriptor,
    result_descriptor: TypedDescriptor,
    input: TransportBuffer,
    admitted: AdmittedSpawn,
}

impl PreparedSpawn {
    fn new(
        module: &IrModule,
        target: &str,
        arguments: &[Value],
        admission: &SpawnAdmissionController,
        owner: &Rc<RefCell<OwnerValues>>,
    ) -> Result<Self> {
        let function = &module.functions[function_index(module, target)?];
        let mut owner = owner.borrow_mut();
        let descriptor = owner.transport_descriptor(&function.parameter_types)?;
        let result_descriptor =
            owner.transport_descriptor(std::slice::from_ref(&function.result_type))?;
        let input = owner.transport_snapshot(&function.parameter_types, arguments)?;
        let mut admitted = admission.admit(OriginDomainId(0), descriptor.preflight(&input)?)?;
        let input = admitted.encode_typed_input(&descriptor, &input)?;
        Ok(Self {
            descriptor,
            result_descriptor,
            input,
            admitted,
        })
    }
}

#[derive(Default)]
struct SharedState {
    gc_waiters: BTreeMap<u64, usize>,
    peak_future_records: usize,
    gc_retired_futures: usize,
    trace_enabled: bool,
    live_main_tasks: std::collections::BTreeSet<u64>,
    main_task_limit: Option<usize>,
    future_admissions: Vec<FutureAdmission>,
    admission_ordinals: HashMap<u64, u64>,
    group_owners: HashMap<u64, u64>,
    accounting: rinzu_runtime::resource_accounting::FutureRecords,
    next_future: u64,
    next_task_group: u64,
    pending: HashMap<u64, PendingRequest>,
    operations: HashMap<u64, &'static str>,
    completed: HashMap<u64, std::result::Result<Value, Failure>>,
    result: Option<std::result::Result<Value, Failure>>,
    internal_error: Option<String>,
    shutdown_requested: bool,
    exit_status: u8,
    control_requests: std::collections::BTreeSet<u64>,
    control_attempts: std::collections::BTreeSet<u64>,
    scope_cancellation_requests: std::collections::BTreeSet<u64>,
    caught_errors: Vec<Failure>,
    last_awaited: Option<Value>,
    awaited_scalars: Vec<ScalarValue>,
    awaited_tasks: BTreeMap<u64, Vec<(u64, ScalarValue)>>,
    events: Vec<SourceAsyncEvent>,
    scopes: BTreeMap<u64, ChildScope>,
    future_scopes: HashMap<u64, Option<u64>>,
    worker_heaps: Vec<SourceWorkerHeapEvidence>,
    worker_returns: usize,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    ready_waiters: HashMap<u64, BTreeMap<u64, rinzu_runtime::ready_work::ReadyPublication>>,
}

impl SharedState {
    fn observe_awaited(&mut self, scope: u64, future: u64, value: Value) {
        if let Some(scalar) = value.scalar() {
            self.awaited_scalars.push(scalar);
            self.awaited_tasks
                .entry(scope)
                .or_default()
                .push((future, scalar));
        }
        self.last_awaited = Some(value);
    }

    fn task_group(&mut self, owner: u64, all: bool) -> Result<u64> {
        let identity = (1_u64 << 63)
            .checked_add(self.next_task_group)
            .ok_or_else(|| anyhow!("task group identity exhausted"))?;
        self.next_task_group = self
            .next_task_group
            .checked_add(1)
            .ok_or_else(|| anyhow!("task group identity exhausted"))?;
        self.scopes.insert(
            identity,
            if all {
                ChildScope::all()
            } else {
                ChildScope::new()
            },
        );
        self.group_owners.insert(identity, owner);
        Ok(identity)
    }
    fn scope(&mut self, identity: u64) -> &mut ChildScope {
        self.scopes.entry(identity).or_default()
    }

    fn admit_with_lease(&mut self, scope: u64, callback: bool) -> Result<u64> {
        let future = self.next_future;
        let parent_scope = self.group_owners.get(&scope).copied().unwrap_or(scope);
        let next = self.admission_ordinals.entry(parent_scope).or_default();
        let ordinal = *next;
        *next = next
            .checked_add(1)
            .ok_or_else(|| anyhow!("admission ordinal exhausted"))?;
        self.future_admissions.push(FutureAdmission {
            future,
            parent_scope,
            ordinal,
        });
        if callback {
            self.scope(scope).admit_callback(future)?;
        } else {
            self.scope(scope).admit(future)?;
        }
        self.next_future += 1;
        self.future_scopes.insert(future, Some(scope));
        self.peak_future_records = self.peak_future_records.max(self.future_scopes.len());
        self.accounting.insert(future);
        Ok(future)
    }

    fn future(&mut self, scope: u64, request: PendingRequest) -> Result<Value> {
        self.future_with_lease(scope, request, false)
    }
    fn future_with_lease(
        &mut self,
        scope: u64,
        request: PendingRequest,
        callback: bool,
    ) -> Result<Value> {
        let (operation, invalid) = match &request {
            PendingRequest::NativeCompletion { transfer, .. } => (
                if *transfer {
                    "native-transfer"
                } else {
                    "native-completion"
                },
                None,
            ),
            PendingRequest::FileReadSize { path, source } => (
                "file-read-size",
                package_path(Path::new("."), path)
                    .is_err()
                    .then_some((ErrorKind::Io, *source)),
            ),
            PendingRequest::Timer {
                milliseconds,
                source,
            } => (
                "timer",
                (*milliseconds < 0).then_some((ErrorKind::Conversion, *source)),
            ),
            PendingRequest::Spawn { .. } => unreachable!("spawn uses prepared admission"),
        };
        if let Some((kind, source)) = invalid {
            return Err(ManagedFailure(Failure::new(Diagnostic {
                cause: FailureCause::Error(kind),
                source,
            }))
            .into());
        }
        let future = self.admit_with_lease(scope, callback)?;
        self.operations.insert(future, operation);
        self.pending.insert(future, request);
        Ok(Value::Future(future))
    }

    fn observe(&mut self, observer_scope: u64, future: u64) -> Result<()> {
        let scope = *self
            .future_scopes
            .get(&future)
            .ok_or_else(|| anyhow!("future {future} has no parent scope"))?;
        let observed_scope = if let Some(scope) = scope
            && !self.scope(scope).is_finished()
        {
            self.scope(scope).observe(future)?;
            Some(scope)
        } else {
            None
        };
        self.events.push(SourceAsyncEvent::Awaited {
            scope: observer_scope,
            future,
        });
        if let Some(scope) = observed_scope {
            // Only the parent ledger ends here. Keep the handle, immutable
            // completion, result roots and physical resources until consume/GC.
            self.scope(scope).retire_observed(future)?;
            self.future_scopes.insert(future, None);
        }
        Ok(())
    }

    fn spawn(
        &mut self,
        scope: u64,
        target: String,
        prepared: Result<PreparedSpawn>,
        source: Option<SourcePosition>,
        callback: bool,
    ) -> Result<Value> {
        let mut prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => match error.downcast_ref::<SpawnAdmissionError>() {
                Some(
                    SpawnAdmissionError::WorkerDomainLimit { .. }
                    | SpawnAdmissionError::InFlightLimit { .. }
                    | SpawnAdmissionError::Transport(
                        TransportError::OperationLimit { .. } | TransportError::ProcessLimit { .. },
                    ),
                ) => {
                    return Err(ManagedFailure(Failure::new(Diagnostic {
                        cause: FailureCause::Overloaded,
                        source,
                    }))
                    .into());
                }
                _ => return Err(error),
            },
        };
        let future = self.admit_with_lease(scope, callback)?;
        prepared.input.set_evidence_operation(future);
        prepared.admitted.set_evidence_operation(future);
        self.operations.insert(future, "spawn");
        self.pending.insert(
            future,
            PendingRequest::Spawn {
                target,
                prepared: Box::new(prepared),
                source,
            },
        );
        Ok(Value::Future(future))
    }
    fn withdraw_admission(&mut self, future: u64) -> Result<()> {
        let scope = self
            .future_scopes
            .remove(&future)
            .flatten()
            .ok_or_else(|| anyhow!("withdrawal without an owning scope"))?;
        self.scope(scope).withdraw_admission(future)?;
        self.pending.remove(&future);
        self.operations.remove(&future);
        self.accounting.remove(future);
        self.future_admissions
            .retain(|admission| admission.future != future);
        Ok(())
    }
    fn complete(
        &mut self,
        future: u64,
        mut value: std::result::Result<Value, Failure>,
    ) -> Result<()> {
        if self.trace_enabled
            && let Err(failure) = &mut value
        {
            failure.mark_unrecorded_traces();
        }
        self.live_main_tasks.remove(&future);
        let scope = *self
            .future_scopes
            .get(&future)
            .ok_or_else(|| anyhow!("future {future} has no parent scope"))?;
        self.scope(scope.ok_or_else(|| anyhow!("completion after task group settlement"))?)
            .complete(future, value.as_ref().err().cloned())?;
        self.completed.insert(future, value);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some(waiters) = self.ready_waiters.remove(&future) {
            for publication in waiters.into_values() {
                publication.publish();
            }
        }
        Ok(())
    }
    fn consume(&mut self, future: u64) -> Result<()> {
        ensure!(
            self.completed.contains_key(&future),
            "consume before physical completion"
        );
        if let Some(Some(scope)) = self.future_scopes.get(&future).copied() {
            self.scope(scope).retire_observed(future)?;
        }
        self.completed.remove(&future);
        self.future_scopes.remove(&future);
        self.operations.remove(&future);
        self.scopes.remove(&(future + 1));
        self.accounting.remove(future);
        Ok(())
    }
    fn retire_unreferenced(&mut self, future: u64) -> Result<bool> {
        if !self.completed.contains_key(&future) {
            return Ok(false);
        }
        if let Some(Some(scope)) = self.future_scopes.get(&future).copied()
            && !self.scope(scope).retire_unreferenced(future)?
        {
            return Ok(false);
        }
        self.completed.remove(&future);
        self.future_scopes.remove(&future);
        self.operations.remove(&future);
        self.scopes.remove(&(future + 1));
        self.accounting.remove(future);
        self.gc_retired_futures += 1;
        Ok(true)
    }
}

struct Frame {
    error_origin: Option<u64>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    native_calls: HashMap<rinzu_compiler::NativeCallId, native::NativeCall>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    supplant_reservations: HashMap<rinzu_compiler::ReservationId, u64>,
    data_roots: Option<OwnerFrame>,
    failure: FailureRoutes,
    function: usize,
    block: BlockId,
    operation: usize,
    values: HashMap<ValueId, Value>,
    locals: HashMap<String, Value>,
    arguments: Vec<Value>,
    waiting: Option<(u64, ValueId)>,
    deferred_return: Option<(BlockId, Value)>,
}

impl Frame {
    fn new(module: &IrModule, function: usize, ir: &IrFunction, arguments: Vec<Value>) -> Self {
        Self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_calls: HashMap::new(),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            supplant_reservations: HashMap::new(),
            data_roots: None,
            error_origin: None,
            failure: FailureRoutes::new(module, ir),
            function,
            block: ir.entry,
            operation: 0,
            values: HashMap::new(),
            locals: HashMap::new(),
            arguments,
            waiting: None,
            deferred_return: None,
        }
    }
    fn jump(&mut self, block: BlockId) {
        self.block = block;
        self.operation = if self
            .deferred_return
            .as_ref()
            .is_some_and(|(return_block, _)| *return_block == block)
        {
            usize::MAX
        } else {
            0
        };
    }
    fn defer_return(
        &mut self,
        block: &rinzu_compiler::BasicBlock,
        value: Option<ValueId>,
    ) -> Result<Option<BlockId>> {
        let Some(target) = self.failure.return_target(block) else {
            return Ok(None);
        };
        self.apply_scope_release()?;
        let value = value
            .map(|id| {
                self.values
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| anyhow!("return value is unavailable"))
            })
            .transpose()?
            .unwrap_or(Value::Unit);
        if let Some(roots) = &self.data_roots {
            roots.hold(&value);
        }
        self.deferred_return = Some((block.id, value));
        Ok(Some(target))
    }
    fn apply_scope_release(&mut self) -> Result<()> {
        if let Some(plan) = self.failure.take_scope_release()
            && let Some(roots) = &self.data_roots
        {
            roots.retire_scopes(&plan)?;
        }
        Ok(())
    }
    fn propagate(
        &mut self,
        operation: &rinzu_compiler::Operation,
        failure: Failure,
    ) -> Result<BlockId> {
        self.error_origin = match operation.kind {
            OperationKind::Await { future, .. } => match self.values.get(&future) {
                Some(Value::Future(id)) => Some(*id),
                _ => None,
            },
            _ => None,
        };
        let target = self.failure.propagate(operation, failure)?;
        if self
            .deferred_return
            .as_ref()
            .is_some_and(|(block, _)| !self.failure.retains_return(*block))
        {
            self.deferred_return = None;
        }
        self.apply_scope_release()?;
        Ok(target)
    }
    fn goto(&mut self, block: &rinzu_compiler::BasicBlock, target: BlockId) -> Result<BlockId> {
        let target = self.failure.goto(block, target)?;
        self.apply_scope_release()?;
        Ok(target)
    }
    fn finish_failure(&mut self) -> Result<Option<Failure>> {
        let failure = self.failure.finish()?;
        if let Some(roots) = &self.data_roots {
            roots.close_scopes()?;
        }
        Ok(failure)
    }
}

struct SourceTask {
    module: Arc<IrModule>,
    native_async: BTreeMap<String, NativeAsyncOperation>,
    frame: Frame,
    shared: Arc<Mutex<SharedState>>,
    owner_thread: ThreadId,
    native_packages: Option<Rc<LoadedNativePackages>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    source_native: Rc<NativeEngine>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    supplants: Weak<RefCell<native::Supplants>>,
    cancellation: CancellationState,
    admission: SpawnAdmissionController,
    continuations: Weak<RefCell<BTreeMap<u64, SourceTask>>>,
    operations: Weak<RefCell<SourceOperations>>,
    scope: u64,
    completion: Option<u64>,
    closing: Option<std::result::Result<Value, Failure>>,
    closing_child: Option<u64>,
    closing_group: Option<bool>,
    task_groups: Vec<TaskGroup>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    readiness: Option<(u64, rinzu_runtime::ready_work::ReadyReceipt)>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    wait_dependency: Option<rinzu_runtime::native_diagnostics::WaitDependency>,
}

#[derive(Debug)]
struct TaskGroup {
    source: rinzu_compiler::SourceRange,
    body: u64,
    cleanup: u64,
    cleaning: bool,
    allow_cancel: bool,
}

// Immutable capsule context: no borrowed SourceTask or active frame crosses
// native entry. Children always use the captured Origin task's scope.
#[derive(Clone)]
struct MainTaskContext {
    module: Arc<IrModule>,
    native_async: BTreeMap<String, NativeAsyncOperation>,
    owner: Rc<RefCell<OwnerValues>>,
    shared: Arc<Mutex<SharedState>>,
    owner_thread: ThreadId,
    native_packages: Option<Rc<LoadedNativePackages>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    source_native: Rc<NativeEngine>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    supplants: Weak<RefCell<native::Supplants>>,
    admission: SpawnAdmissionController,
    continuations: Weak<RefCell<BTreeMap<u64, SourceTask>>>,
    operations: Weak<RefCell<SourceOperations>>,
    scope: u64,
    child_scope: u64,
    cleanup: bool,
    callback_admission: bool,
}

#[derive(Clone, Default)]
struct CancellationState {
    polls: Rc<Cell<usize>>,
    requested: Rc<Cell<bool>>,
    shutdown_requested: Rc<Cell<bool>>,
    after_polls: Option<usize>,
    shutdown_after_polls: Option<usize>,
    masked: bool,
}

impl CancellationState {
    fn new(after_polls: Option<usize>) -> Self {
        Self {
            after_polls,
            ..Self::default()
        }
    }
    fn child(&self) -> Self {
        Self {
            polls: Rc::clone(&self.polls),
            after_polls: self.after_polls,
            shutdown_after_polls: self.shutdown_after_polls,
            requested: Rc::new(Cell::new(false)),
            shutdown_requested: Rc::new(Cell::new(false)),
            masked: self.masked,
        }
    }
    fn callback(&self) -> Self {
        Self {
            polls: Rc::clone(&self.polls),
            requested: Rc::clone(&self.requested),
            shutdown_requested: Rc::clone(&self.shutdown_requested),
            after_polls: self.after_polls,
            shutdown_after_polls: self.shutdown_after_polls,
            masked: true,
        }
    }
    fn poll(&self, in_cleanup: bool, externally_requested: bool) -> bool {
        self.polls.set(self.polls.get().saturating_add(1));
        if self
            .shutdown_after_polls
            .is_some_and(|limit| self.polls.get() >= limit)
        {
            self.shutdown_requested.set(true);
            self.requested.set(true);
        }
        if externally_requested
            || self
                .after_polls
                .is_some_and(|limit| self.polls.get() >= limit)
        {
            self.requested.set(true);
        }
        self.requested.get() && !in_cleanup && !self.masked
    }
    fn cause(&self) -> FailureCause {
        if self.shutdown_requested.get() {
            FailureCause::Shutdown
        } else {
            FailureCause::Cancelled
        }
    }
}

impl ManagedTask for SourceTask {
    fn resume(&mut self, event: TaskEvent) -> TaskAction {
        let previous_wait = self.waiting_future();
        if self
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .shutdown_requested
        {
            self.cancellation.shutdown_requested.set(true);
            self.cancellation.requested.set(true);
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        drop(self.wait_dependency.take());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some((future, receipt)) = self.readiness.take() {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(waiters) = shared.ready_waiters.get_mut(&future) {
                if waiters.remove(&self.scope).is_none() {
                    data::native_fatal(86);
                }
                if waiters.is_empty() {
                    shared.ready_waiters.remove(&future);
                }
            }
            receipt.receive();
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let _activity = self.frame.data_roots.as_ref().map(|roots| {
            roots.owner.borrow().note_owner_turn();
            let function = &self.module.functions[self.frame.function];
            let source = self
                .frame
                .failure
                .diagnostic(FailureCause::RuntimePanic, function.range)
                .source;
            let activity = roots.owner.borrow().prepare_activity(
                rinzu_runtime::native_diagnostics::ActivityKind::Managed,
                &function.name,
                source,
            );
            activity.start();
            activity
        });
        let function = &self.module.functions[self.frame.function];
        let _error_trace = self.frame.failure.tracer().task(
            &function.name,
            self.frame
                .failure
                .diagnostic(FailureCause::RuntimePanic, function.range)
                .source,
        );
        let action = (|| {
            let result = if self.closing.is_some() {
                self.resume_close(event)
            } else {
                self.resume_inner(event)
            };
            match result {
                Ok(action) => action,
                Err(error) => {
                    if self.closing.is_none()
                        && let Some(failure) = error.downcast_ref::<ManagedFailure>()
                    {
                        return self
                            .begin_close(Err(failure.0.clone()))
                            .unwrap_or_else(|error| TaskAction::Fail(format!("{error:#}")));
                    }
                    let mut shared = self
                        .shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(failure) = error.downcast_ref::<ManagedFailure>() {
                        shared.result = Some(Err(failure.0.clone()));
                        if failure.0.primary.cause == FailureCause::Cancelled {
                            return TaskAction::Cancelled;
                        }
                    } else {
                        shared.internal_error = Some(format!("{error:#}"));
                    }
                    TaskAction::Fail(format!("{error:#}"))
                }
            }
        })();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if matches!(action, TaskAction::Pending { .. })
            && let Some(roots) = &self.frame.data_roots
        {
            let future = self
                .waiting_future()
                .unwrap_or_else(|| data::native_fatal(86));
            let function = &self.module.functions[self.frame.function];
            let source = self
                .frame
                .failure
                .diagnostic(FailureCause::RuntimePanic, function.range)
                .source;
            self.wait_dependency = Some(
                roots.owner.borrow().prepare_wait(
                    self.scope,
                    future
                        .checked_add(1)
                        .unwrap_or_else(|| data::native_fatal(86)),
                    &function.name,
                    source,
                ),
            );
            let receipt = roots
                .owner
                .borrow()
                .prepare_readiness(&function.name, source);
            let publication = receipt.publication();
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if shared.completed.contains_key(&future) {
                publication.publish();
            } else if shared
                .ready_waiters
                .entry(future)
                .or_default()
                .insert(self.scope, publication)
                .is_some()
            {
                data::native_fatal(86);
            }
            self.readiness = Some((future, receipt));
        }
        {
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(future) = previous_wait {
                let pins = shared
                    .gc_waiters
                    .get_mut(&future)
                    .ok_or_else(|| anyhow!("missing Future wait pin"));
                match pins {
                    Ok(count) => {
                        *count -= 1;
                        if *count == 0 {
                            shared.gc_waiters.remove(&future);
                        }
                    }
                    Err(_) => data::native_fatal(86),
                }
            }
            if matches!(action, TaskAction::Pending { .. })
                && let Some(future) = self.waiting_future()
            {
                *shared.gc_waiters.entry(future).or_default() += 1;
            }
        }
        action
    }
}

impl MainTaskContext {
    fn source_service(
        &self,
        signature: &rinzu_compiler::SourceNativeSignature,
        arguments: Vec<Value>,
        source: Option<SourcePosition>,
    ) -> Result<Value> {
        let rinzu_compiler::ast::NativeBinding::Runtime(function) = &signature.binding else {
            bail!("async native call has no closed service")
        };
        let request = match function {
            rinzu_compiler::RuntimeFunction::NativeCompleted
            | rinzu_compiler::RuntimeFunction::NativeTransferred => {
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                {
                    let rinzu_compiler::SourceAbiType::Opaque(identity) = &signature.parameters[0]
                    else {
                        bail!("completion protocol requires an opaque SDK handle");
                    };
                    let address = self
                        .owner
                        .borrow()
                        .native_opaque_pointer(&arguments[0], identity)?;
                    // SAFETY: the trusted SDK service pins native_completion.h v1
                    // metadata through physical completion and every admitted wait.
                    let completion = unsafe {
                        rinzu_runtime::native_completion::CompletionPointer::new(address as usize)
                    };
                    let transfer = *function == rinzu_compiler::RuntimeFunction::NativeTransferred;
                    if transfer {
                        completion.require_transfer();
                    }
                    PendingRequest::NativeCompletion {
                        completion,
                        transfer,
                        source,
                    }
                }
                #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
                bail!("native completion SDK requires the supported native target")
            }
            rinzu_compiler::RuntimeFunction::ReadResourceSize => {
                let Some(Value::String(path)) = arguments.first() else {
                    bail!("file service argument is not a String")
                };
                PendingRequest::FileReadSize {
                    path: path.clone(),
                    source,
                }
            }
            rinzu_compiler::RuntimeFunction::Sleep => PendingRequest::Timer {
                milliseconds: arguments
                    .first()
                    .context("timer argument is missing")?
                    .as_i64()?,
                source,
            },
        };
        let future = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .future_with_lease(self.child_scope, request, self.callback_admission)?;
        self.start_child(future)
    }

    fn task_control(
        &self,
        kind: rinzu_compiler::TaskControlKind,
        value: Option<&Value>,
    ) -> Result<Value> {
        use rinzu_compiler::TaskControlKind as K;
        ensure!(
            std::thread::current().id() == self.owner_thread,
            "foreign task control"
        );
        if kind == K::ExitStatus {
            let status = value
                .ok_or_else(|| anyhow!("missing process status"))?
                .as_i64()?;
            let valid = (0..=255).contains(&status);
            if valid {
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .exit_status = status as u8;
            }
            return Ok(Value::Scalar(ScalarValue::from_bool(valid)));
        }
        if kind == K::Shutdown {
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            let first = !shared.shutdown_requested;
            shared.shutdown_requested = true;
            drop(shared);
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            self.owner.borrow().note_shutdown_request();
            return Ok(Value::Scalar(ScalarValue::from_bool(first)));
        }
        let Some(Value::Future(future)) = value else {
            bail!("task control requires Future")
        };
        let future = *future;
        let operations = self
            .operations
            .upgrade()
            .ok_or_else(|| anyhow!("services ended"))?;
        if kind == K::Ready {
            let _ = operations.borrow_mut().receive_one_ready()?;
        }
        let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        ensure!(
            shared.future_scopes.contains_key(&future),
            "unknown Future identity"
        );
        let ready = shared.completed.contains_key(&future);
        shared.control_attempts.insert(future);
        if kind == K::Ready {
            return Ok(Value::Scalar(ScalarValue::from_bool(ready)));
        }
        if ready || shared.control_requests.contains(&future) {
            return Ok(Value::Scalar(ScalarValue::from_bool(false)));
        }
        drop(shared);
        let won = if operations.borrow().active.contains_key(&future)
            || operations.borrow().deferred_native.contains(&future)
        {
            operations
                .borrow()
                .request_control(future, kind == K::Timeout)
        } else {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let native = self
                .supplants
                .upgrade()
                .ok_or_else(|| anyhow!("supplants ended"))?
                .borrow()
                .cancel(future, rinzu_runtime::supplant::Cancellation::Task);
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            let native = false;
            if native {
                true
            } else {
                let tasks = self
                    .continuations
                    .upgrade()
                    .ok_or_else(|| anyhow!("tasks ended"))?;
                let mut child = tasks
                    .borrow_mut()
                    .remove(&future)
                    .ok_or_else(|| anyhow!("pending Future has no operation"))?;
                // Explicit control targets this child even when it inherited
                // a callback mask; its finally frames still mask cleanup.
                child.cancellation.masked = false;
                park_continuation(&tasks, child, TaskEvent::CancelRequested)?;
                true
            }
        };
        if won {
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            shared.control_requests.insert(future);
            if let Some(Some(scope)) = shared.future_scopes.get(&future).copied() {
                shared.scope(scope).expect_control(future)?;
            }
        }
        Ok(Value::Scalar(ScalarValue::from_bool(won)))
    }

    fn start_child(&self, value: Value) -> Result<Value> {
        let Value::Future(future) = value else {
            bail!("child start requires a Future")
        };
        self.operations
            .upgrade()
            .ok_or_else(|| anyhow!("child services ended"))?
            .borrow_mut()
            .start(future)?;
        Ok(Value::Future(future))
    }
}

impl SourceTask {
    fn child_scope(&self) -> u64 {
        self.task_groups.last().map_or(self.scope, |group| {
            if group.cleaning {
                group.cleanup
            } else {
                group.body
            }
        })
    }
    fn join_scope(&self) -> u64 {
        if self.closing_group.is_some() {
            self.child_scope()
        } else {
            self.scope
        }
    }
    fn begin_group_join(
        &mut self,
        source: rinzu_compiler::SourceRange,
        cleanup: bool,
    ) -> Result<TaskAction> {
        let group = self
            .task_groups
            .last()
            .ok_or_else(|| anyhow!("task scope join without enter"))?;
        ensure!(
            group.source == source && group.cleaning == cleanup,
            "task scope join order mismatch"
        );
        let incoming = self.frame.failure.cleanup_incoming();
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .scope(self.child_scope())
            .begin_close(incoming.clone())?;
        self.closing = Some(incoming.map_or(Ok(Value::Unit), Err));
        self.closing_group = Some(cleanup);
        self.resume_close(TaskEvent::Start)
    }
    fn main_context(&self) -> MainTaskContext {
        MainTaskContext {
            module: Arc::clone(&self.module),
            native_async: self.native_async.clone(),
            owner: Rc::clone(&self.frame.data_roots.as_ref().expect("Main Owner").owner),
            shared: Arc::clone(&self.shared),
            owner_thread: self.owner_thread,
            native_packages: self.native_packages.clone(),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            source_native: Rc::clone(&self.source_native),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            supplants: self.supplants.clone(),
            admission: self.admission.clone(),
            continuations: self.continuations.clone(),
            operations: self.operations.clone(),
            scope: self.scope,
            child_scope: self.child_scope(),
            cleanup: self.frame.failure.in_cleanup(),
            callback_admission: false,
        }
    }
    fn start_child(&self, value: Value) -> Result<Value> {
        self.main_context().start_child(value)
    }
    fn source_service(
        &self,
        signature: &rinzu_compiler::SourceNativeSignature,
        arguments: Vec<Value>,
        source: Option<SourcePosition>,
    ) -> Result<Value> {
        self.main_context()
            .source_service(signature, arguments, source)
    }

    fn begin_close(&mut self, outcome: std::result::Result<Value, Failure>) -> Result<TaskAction> {
        if let (Some(roots), Ok(value)) = (&self.frame.data_roots, &outcome) {
            roots.hold(value);
        }
        if self.completion.is_some() && outcome.as_ref().err().is_none_or(Failure::recoverable) {
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            let mut children = shared.scopes.remove(&self.scope).unwrap_or_default();
            children.handoff(shared.scope(0))?;
            shared.scopes.insert(self.scope, children);
            for parent in shared.future_scopes.values_mut() {
                if *parent == Some(self.scope) {
                    *parent = Some(0);
                }
            }
        }
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .scope(self.scope)
            .begin_close(outcome.as_ref().err().cloned())?;
        self.closing = Some(outcome);
        self.resume_close(TaskEvent::Start)
    }

    fn resume_close(&mut self, event: TaskEvent) -> Result<TaskAction> {
        if self
            .shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .shutdown_requested
        {
            self.cancellation.shutdown_requested.set(true);
            self.cancellation.requested.set(true);
        }
        let scope = self.join_scope();
        if (matches!(
            event,
            TaskEvent::CancelRequested | TaskEvent::ShutdownRequested { .. }
        ) || self.cancellation.requested.get())
            && self.closing.as_ref().is_some_and(Result::is_ok)
            && !self.cancellation.masked
            && self.closing_group.is_none_or(|cleanup| {
                !cleanup && self.task_groups.last().is_some_and(|g| g.allow_cancel)
            })
        {
            let cause = if matches!(event, TaskEvent::ShutdownRequested { .. })
                || self.cancellation.shutdown_requested.get()
            {
                FailureCause::Shutdown
            } else {
                FailureCause::Cancelled
            };
            let range = if self.completion.is_none() {
                self.task_groups
                    .last()
                    .map_or(self.module.functions[self.frame.function].range, |g| {
                        g.source
                    })
            } else {
                self.module.functions[self.frame.function].range
            };
            let source = self.frame.failure.diagnostic(cause, range).source;
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if cause == FailureCause::Shutdown {
                shared.scope(scope).shutdown_close(source)?;
            } else {
                shared.scope(scope).cancel_close(source)?;
            }
            self.closing = Some(Err(Failure::new(Diagnostic { cause, source })));
        }
        if let TaskEvent::Operation(_) = event {
            let future = self
                .closing_child
                .take()
                .ok_or_else(|| anyhow!("scope close resumed without a child"))?;
            ensure!(
                self.shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .completed
                    .contains_key(&future),
                "scope close resumed before child completion"
            );
        }
        let outcome = self
            .closing
            .clone()
            .ok_or_else(|| anyhow!("scope is not closing"))?;
        let pending = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .scope(scope)
            .pending()?;
        if outcome.as_ref().err().is_some_and(|failure| {
            self.shared
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .scope(scope)
                .cancels_on_failure(failure)
        }) {
            let shutdown = outcome
                .as_ref()
                .err()
                .is_some_and(|failure| failure.primary.cause == FailureCause::Shutdown);
            for future in pending {
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .scope_cancellation_requests
                    .insert(future);
                let cancelled = self
                    .operations
                    .upgrade()
                    .ok_or_else(|| anyhow!("child services ended"))?
                    .borrow()
                    .cancel(future);
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let cancelled = cancelled
                    || self
                        .supplants
                        .upgrade()
                        .ok_or_else(|| anyhow!("supplant registry ended"))?
                        .borrow()
                        .cancel(
                            future,
                            if shutdown {
                                rinzu_runtime::supplant::Cancellation::Shutdown
                            } else {
                                rinzu_runtime::supplant::Cancellation::Task
                            },
                        );
                if !cancelled {
                    let tasks = self
                        .continuations
                        .upgrade()
                        .ok_or_else(|| anyhow!("continuation registry ended"))?;
                    let child = tasks
                        .borrow_mut()
                        .remove(&future)
                        .ok_or_else(|| anyhow!("pending child has no continuation"))?;
                    park_continuation(
                        &tasks,
                        child,
                        if shutdown {
                            TaskEvent::ShutdownRequested { exit_code: 0 }
                        } else {
                            TaskEvent::CancelRequested
                        },
                    )?;
                }
            }
        }
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(future) = shared.scope(scope).pending()?.first() {
            self.closing_child = Some(*future);
            return Ok(TaskAction::Pending {
                label: format!("source-future:{future}"),
                cleanup_safe: true,
            });
        }
        let failure = shared.scope(scope).finish_close()?;
        if let Some(cleanup) = self.closing_group.take() {
            self.frame
                .failure
                .settle_task_join(self.task_groups.last().unwrap().source, failure)?;
            if cleanup {
                let group = self.task_groups.pop().unwrap();
                for parent in shared.future_scopes.values_mut() {
                    if *parent == Some(group.body) || *parent == Some(group.cleanup) {
                        *parent = None;
                    }
                }
                shared.scopes.remove(&group.body);
                shared.scopes.remove(&group.cleanup);
                shared.group_owners.remove(&group.body);
                shared.group_owners.remove(&group.cleanup);
            } else {
                self.task_groups.last_mut().unwrap().cleaning = true;
            }
            self.closing = None;
            self.closing_child = None;
            drop(shared);
            return self.resume_inner(TaskEvent::Start);
        }
        let result = match failure {
            Some(failure) => Err(failure),
            None => outcome,
        };
        let action = match &result {
            Ok(_) => TaskAction::Return,
            Err(failure) if failure.primary.cause == FailureCause::Cancelled => {
                TaskAction::Cancelled
            }
            Err(failure) => TaskAction::Fail(failure_message(failure)),
        };
        // The child ledger has physically settled. Returned/aliased Futures
        // keep their immutable result independently of the finished task.
        for parent in shared.future_scopes.values_mut() {
            if *parent == Some(self.scope) {
                *parent = None;
            }
        }
        shared.scopes.remove(&self.scope);
        shared.admission_ordinals.remove(&self.scope);
        if let Some(future) = self.completion {
            if let (Some(roots), Ok(value)) = (&self.frame.data_roots, &result) {
                roots.owner.borrow_mut().retain_result(future, value);
            }
            shared.complete(future, result)?;
        } else {
            shared.result = Some(result);
        }
        Ok(action)
    }

    fn waiting_future(&self) -> Option<u64> {
        self.closing_child
            .or_else(|| self.frame.waiting.map(|waiting| waiting.0))
    }

    fn ready_event(&self, shared: &SharedState) -> Option<TaskEvent> {
        match self.waiting_future() {
            None => Some(TaskEvent::Start),
            Some(future) if shared.completed.contains_key(&future) => {
                Some(TaskEvent::Operation(Resolution::Completed(Ok(()))))
            }
            Some(_) => None,
        }
    }

    fn resume_inner(&mut self, event: TaskEvent) -> Result<TaskAction> {
        match event {
            TaskEvent::Start | TaskEvent::Yielded => {}
            TaskEvent::Operation(resolution) => {
                let (future, result_id) = self
                    .frame
                    .waiting
                    .take()
                    .ok_or_else(|| anyhow!("continuation resumed without an awaited future"))?;
                let completed = self
                    .shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .completed
                    .get(&future)
                    .cloned()
                    .ok_or_else(|| anyhow!("future {future} completed without a value"))?;
                let completed = match resolution {
                    Resolution::Completed(_) => completed,
                    Resolution::Cancelled | Resolution::TimedOut => {
                        self.shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .events
                            .push(SourceAsyncEvent::Cancelled { future });
                        // A cancellation winner cannot publish a late success. Preserve
                        // cancellation cleanup after the Worker has fully settled.
                        Err(match completed {
                            Err(failure) if failure.primary.cause == FailureCause::Cancelled => {
                                failure
                            }
                            _ => {
                                let operation = self.awaited_operation()?;
                                Failure::new(
                                    self.frame
                                        .failure
                                        .diagnostic(FailureCause::Cancelled, operation.range),
                                )
                            }
                        })
                    }
                };
                self.shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .observe(self.scope, future)?;
                match completed {
                    Ok(value) => {
                        self.shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .observe_awaited(self.scope, future, value.clone());
                        let operation = self.awaited_operation()?;
                        self.retire_awaited_supplant(&operation, future, Some(&value))?;
                        self.frame.store_result(
                            result_id,
                            value,
                            &operation.result.as_ref().unwrap().ty,
                        )?;
                        self.consume_awaited(&operation, future)?;
                    }
                    Err(failure) => {
                        let operation = self.awaited_operation()?;
                        self.retire_awaited_supplant(&operation, future, None)?;
                        self.consume_awaited(&operation, future)?;
                        let target = self.frame.propagate(&operation, failure)?;
                        self.jump(target);
                    }
                }
                self.shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .events
                    .push(SourceAsyncEvent::Resumed {
                        scope: self.scope,
                        future,
                        owner_thread: std::thread::current().id(),
                    });
            }
            TaskEvent::CancelRequested | TaskEvent::ShutdownRequested { .. } => {
                self.cancellation.requested.set(true);
                if matches!(event, TaskEvent::ShutdownRequested { .. }) {
                    self.cancellation.shutdown_requested.set(true);
                }
                if (self.cancellation.masked || self.frame.failure.in_cleanup())
                    && let Some((future, _)) = self.frame.waiting
                {
                    // Keep the awaited value and resume only after physical
                    // completion when inherited control is masked.
                    return Ok(TaskAction::Pending {
                        label: format!("source-future:{future}"),
                        cleanup_safe: true,
                    });
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some((future, _)) = self.frame.waiting
                    && matches!(
                        self.awaited_operation()?.kind,
                        OperationKind::SettleSupplant { .. }
                    )
                {
                    self.supplants
                        .upgrade()
                        .ok_or_else(|| anyhow!("supplant registry ended"))?
                        .borrow()
                        .cancel(
                            future,
                            if matches!(event, TaskEvent::ShutdownRequested { .. }) {
                                rinzu_runtime::supplant::Cancellation::Shutdown
                            } else {
                                rinzu_runtime::supplant::Cancellation::Task
                            },
                        );
                    return Ok(TaskAction::Pending {
                        label: format!("source-future:{future}"),
                        cleanup_safe: false,
                    });
                }
                if self.frame.waiting.take().is_some() {
                    let operation = self.awaited_operation()?;
                    let failure = Failure::new(
                        self.frame
                            .failure
                            .diagnostic(self.cancellation.cause(), operation.range),
                    );
                    let target = self.frame.propagate(&operation, failure)?;
                    self.jump(target);
                }
            }
            TaskEvent::ChildSpawned(_) => bail!("IR continuation does not create scheduler tasks"),
        }

        loop {
            let (operation, terminal_block) = {
                let function = &self.module.functions[self.frame.function];
                let block = function
                    .blocks
                    .iter()
                    .find(|block| block.id == self.frame.block)
                    .ok_or_else(|| anyhow!("missing IR block {:?}", self.frame.block))?;
                if self.frame.operation == 0 {
                    self.frame.failure.enter(block);
                }
                let operation = block.operations.get(self.frame.operation).cloned();
                let terminal = operation.is_none().then(|| block.clone());
                (operation, terminal)
            };
            if let Some(operation) = operation {
                self.frame.operation += 1;
                self.frame.failure.before(&operation);
                let result = match self.evaluate_operation(&operation) {
                    Ok(result) => result,
                    Err(error) => {
                        let Some(failure) = error.downcast_ref::<ManagedFailure>() else {
                            return Err(error);
                        };
                        let target = self.frame.propagate(&operation, failure.0.clone())?;
                        self.jump(target);
                        continue;
                    }
                };
                if let Some(action) = result.action {
                    return Ok(action);
                }
                if let (Some(destination), Some(value)) = (&operation.result, result.value) {
                    self.frame
                        .store_result(destination.id, value, &destination.ty)?;
                    if let OperationKind::Await {
                        future,
                        consume: true,
                        ..
                    } = operation.kind
                    {
                        let Value::Future(future) = self.value(future)? else {
                            bail!("invalid consumed future")
                        };
                        self.consume_awaited(&operation, *future)?;
                    }
                }
                continue;
            }
            let block = terminal_block.unwrap();
            match block.terminator {
                Terminator::Goto { target, .. } => {
                    let target = self.frame.goto(&block, target)?;
                    self.jump(target);
                }
                Terminator::Branch {
                    condition,
                    then_target,
                    else_target,
                } => {
                    let condition = self.value(condition)?;
                    self.jump(if truthy(condition)? {
                        then_target
                    } else {
                        else_target
                    });
                }
                Terminator::Return { value, .. } => {
                    if let Some(target) = self.frame.defer_return(&block, value)? {
                        self.jump(target);
                        continue;
                    }
                    if let Some(failure) = self.frame.finish_failure()? {
                        return Err(ManagedFailure(failure).into());
                    }
                    let value = match self.frame.deferred_return.take() {
                        Some((_, value)) => value,
                        None => value
                            .map(|value| self.value(value).cloned())
                            .transpose()?
                            .unwrap_or(Value::Unit),
                    };
                    return self.begin_close(Ok(value));
                }
                Terminator::Fail => {
                    let failure = self
                        .frame
                        .finish_failure()?
                        .ok_or_else(|| anyhow!("source failure block has no failure"))?;
                    return Err(ManagedFailure(failure).into());
                }
                Terminator::Pending | Terminator::Unreachable => {
                    bail!("invalid terminal block in executable continuation")
                }
                Terminator::Switch { value, cases, .. } => {
                    let variant = self
                        .frame
                        .data_roots
                        .as_ref()
                        .ok_or_else(|| anyhow!("missing Main Owner"))?
                        .owner
                        .borrow()
                        .enum_variant(self.value(value)?)?;
                    self.jump(
                        cases
                            .iter()
                            .find(|(tag, _)| *tag == variant)
                            .ok_or_else(|| anyhow!("enum has no matching case"))?
                            .1,
                    );
                }
            }
        }
    }

    fn awaited_operation(&self) -> Result<rinzu_compiler::Operation> {
        self.module.functions[self.frame.function]
            .blocks
            .iter()
            .find(|block| block.id == self.frame.block)
            .and_then(|block| {
                self.frame
                    .operation
                    .checked_sub(1)
                    .and_then(|index| block.operations.get(index))
            })
            .filter(|operation| {
                matches!(
                    operation.kind,
                    OperationKind::Await { .. } | OperationKind::SettleSupplant { .. }
                )
            })
            .cloned()
            .ok_or_else(|| anyhow!("continuation has no active await operation"))
    }

    fn retire_awaited_supplant(
        &mut self,
        _operation: &rinzu_compiler::Operation,
        _future: u64,
        _value: Option<&Value>,
    ) -> Result<()> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let OperationKind::SettleSupplant { reservation, .. } = _operation.kind {
            ensure!(
                self.frame.supplant_reservations.remove(&reservation) == Some(_future),
                "supplant result consumed twice"
            );
            if let Some(value) = _value {
                self.frame
                    .data_roots
                    .as_ref()
                    .expect("Main Owner")
                    .hold(value);
            }
            self.supplants
                .upgrade()
                .ok_or_else(|| anyhow!("supplant registry ended"))?
                .borrow_mut()
                .retire_result(_future);
        }
        Ok(())
    }

    fn consume_awaited(
        &mut self,
        operation: &rinzu_compiler::Operation,
        future: u64,
    ) -> Result<()> {
        if !matches!(operation.kind, OperationKind::Await { consume: true, .. }) {
            return Ok(());
        }
        self.shared
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .consume(future)?;
        if let Some(roots) = &self.frame.data_roots {
            roots.owner.borrow_mut().release_result(future);
        }
        Ok(())
    }
    fn await_future(&mut self, operation: &rinzu_compiler::Operation, future: u64) -> Result<Step> {
        let completed = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .completed
            .get(&future)
            .cloned();
        if let Some(completed) = completed {
            self.shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .observe(self.scope, future)?;
            self.retire_awaited_supplant(operation, future, completed.as_ref().ok())?;
            if completed.is_err() {
                self.consume_awaited(operation, future)?;
            }
            let value = completed.map_err(ManagedFailure)?;
            self.shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .observe_awaited(self.scope, future, value.clone());
            return Ok(Step::value(value));
        }
        let result = operation
            .result
            .as_ref()
            .ok_or_else(|| anyhow!("await has no result slot"))?
            .id;
        self.frame.waiting = Some((future, result));
        let operation_name = {
            let shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match shared.operations.get(&future) {
                Some(operation) => *operation,
                None if self
                    .continuations
                    .upgrade()
                    .is_some_and(|tasks| tasks.borrow().contains_key(&future)) =>
                {
                    "async"
                }
                None => bail!("future {future} has neither result nor operation"),
            }
            .to_owned()
        };
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .push(SourceAsyncEvent::Suspended {
                scope: self.scope,
                future,
                operation: operation_name,
                owner_thread: self.owner_thread,
            });
        Ok(Step::action(TaskAction::Pending {
            label: format!("source-future:{future}"),
            cleanup_safe: self.cancellation.masked || self.frame.failure.in_cleanup(),
        }))
    }

    fn evaluate_operation(&mut self, operation: &rinzu_compiler::Operation) -> Result<Step> {
        let resolved = resolve_indirect(operation, &self.frame.values)?;
        let operation = resolved.as_ref();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let main_context = matches!(operation.kind, OperationKind::PrepareNativeCall { .. })
            .then(|| self.main_context());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if let Some(value) = self.frame.native_operation(
            operation,
            &self.source_native,
            &self.cancellation,
            main_context.as_ref(),
        )? {
            let errors = self
                .frame
                .data_roots
                .as_ref()
                .unwrap()
                .owner
                .borrow_mut()
                .take_callback_errors();
            self.shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .caught_errors
                .extend(errors);
            return Ok(Step::value(value));
        }
        if let Some(value) = self.frame.data_operation(operation)? {
            return Ok(Step::value(value));
        }
        let value = match &operation.kind {
            OperationKind::TaskControl { kind, future } => self
                .main_context()
                .task_control(*kind, future.map(|id| self.value(id)).transpose()?)?,
            OperationKind::CoreLength {
                operand,
                kind: rinzu_compiler::CoreLengthKind::Utf8,
            } => utf8_length(self.value(*operand)?)?,
            OperationKind::ErrorMessage { error } => {
                let Value::Error(failure) = self.value(*error)? else {
                    bail!("Error.message operand is not Error")
                };
                Value::String(failure_message(failure))
            }
            OperationKind::Stdout { .. }
            | OperationKind::FormatString { .. }
            | OperationKind::CoreLength { .. }
            | OperationKind::DataOperation { .. }
            | OperationKind::BytesPad { .. }
            | OperationKind::ErrorIs { .. }
            | OperationKind::ErrorCast { .. }
            | OperationKind::ErrorField { .. }
            | OperationKind::ErrorMatch { .. }
            | OperationKind::ErrorIndex { .. }
            | OperationKind::ErrorRest { .. }
            | OperationKind::ThrowError { .. } => {
                bail!("fixed Bytes execution requires the R3 data backend")
            }
            OperationKind::EnterScope { .. } | OperationKind::ExitScope { .. } => Value::Unit,
            OperationKind::PrepareNativeCall { .. }
            | OperationKind::CloseNativeCall { .. }
            | OperationKind::SettleNativeAdmissions { .. }
            | OperationKind::ReadNativeResult { .. }
            | OperationKind::ReleaseNativeCall { .. } => {
                bail!("native callback ledger execution requires R4 backend")
            }
            OperationKind::CatchErrors { .. } => {
                let failure = self.frame.failure.catch(self.frame.block)?;
                self.shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .caught_errors
                    .push(failure.diagnostics_only());
                self.frame
                    .data_roots
                    .as_ref()
                    .ok_or_else(|| anyhow!("List catch requires managed Owner values"))?
                    .owner
                    .borrow_mut()
                    .error_list(&[failure])?
            }
            OperationKind::CatchError { .. } => {
                let failure = self.frame.failure.catch(self.frame.block)?;
                self.shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .caught_errors
                    .push(failure.diagnostics_only());
                Value::Error(failure)
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            OperationKind::ReserveSupplant { .. } => {
                let context = self.main_context();
                self.supplants
                    .upgrade()
                    .ok_or_else(|| anyhow!("supplant registry ended"))?
                    .borrow_mut()
                    .reserve(&mut self.frame, operation, context, &self.cancellation)?;
                Value::Unit
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            OperationKind::StartSupplant { reservation, .. } => {
                let future = *self
                    .frame
                    .supplant_reservations
                    .get(reservation)
                    .ok_or_else(|| anyhow!("supplant start lost its reservation"))?;
                self.supplants
                    .upgrade()
                    .ok_or_else(|| anyhow!("supplant registry ended"))?
                    .borrow()
                    .queue(future)?;
                Value::Unit
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            OperationKind::SettleSupplant { reservation, .. } => {
                let future = *self
                    .frame
                    .supplant_reservations
                    .get(reservation)
                    .ok_or_else(|| anyhow!("supplant settlement lost its reservation"))?;
                return self.await_future(operation, future);
            }
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            OperationKind::ReserveSupplant { .. }
            | OperationKind::StartSupplant { .. }
            | OperationKind::SettleSupplant { .. } => {
                bail!("supplant execution requires R4 reservation backend")
            }
            OperationKind::FunctionReference { target } => Value::FunctionReference(target.clone()),
            OperationKind::IndirectCall { .. } => unreachable!("resolved typed indirect call"),
            OperationKind::SourceNativeCall {
                call: None,
                signature,
                arguments,
                ..
            } => {
                let arguments = self
                    .arguments(arguments)?
                    .iter()
                    .zip(&signature.parameters)
                    .map(|(value, ty)| {
                        if matches!(ty, rinzu_compiler::SourceAbiType::Opaque(_)) {
                            Ok(value.clone())
                        } else {
                            self.frame.export(value)
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.source_service(
                    signature,
                    arguments,
                    self.frame
                        .failure
                        .diagnostic(FailureCause::Error(ErrorKind::Io), operation.range)
                        .source,
                )?
            }
            OperationKind::SourceNativeCall { .. } => {
                bail!("source native execution requires R4 physical ABI backend")
            }
            OperationKind::NumericConvert { operand, .. } => {
                let value = self
                    .value(*operand)?
                    .scalar()
                    .ok_or_else(|| anyhow!("numeric conversion operand is not scalar"))?;
                let target = crate::scalar_backend::kind(
                    operation.result.as_ref().unwrap().ty.scalar().unwrap(),
                );
                Value::Scalar(value.convert(target).map_err(|_| {
                    ManagedFailure(Failure::new(self.frame.failure.diagnostic(
                        FailureCause::Error(ErrorKind::Conversion),
                        operation.range,
                    )))
                })?)
            }
            OperationKind::Parameter { index } => self
                .frame
                .arguments
                .get(usize::try_from(*index)?)
                .cloned()
                .ok_or_else(|| anyhow!("missing argument {index}"))?,
            OperationKind::AsyncStart
            | OperationKind::EnterTry { .. }
            | OperationKind::LeaveTry { .. }
            | OperationKind::GcSafepoint { .. } => return Ok(Step::none()),
            OperationKind::CancellationPoll { .. } => {
                if self
                    .shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .shutdown_requested
                {
                    self.cancellation.shutdown_requested.set(true);
                    self.cancellation.requested.set(true);
                }

                let cancelled = self
                    .cancellation
                    .poll(self.frame.failure.in_cleanup(), false);
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if self.cancellation.requested.get()
                    && let Some(roots) = &self.frame.data_roots
                {
                    roots.owner.borrow().note_shutdown_request();
                }
                if cancelled {
                    return Err(ManagedFailure(Failure::new(
                        self.frame
                            .failure
                            .diagnostic(self.cancellation.cause(), operation.range),
                    ))
                    .into());
                }
                return Ok(Step::none());
            }
            OperationKind::Constant(value) => {
                constant(value, &operation.result.as_ref().unwrap().ty)?
            }
            OperationKind::LoadLocal { name } => self
                .frame
                .locals
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("read of uninitialized local {name}"))?,
            OperationKind::DeclareLocal { name } => {
                self.frame.locals.remove(name);
                return Ok(Step::none());
            }
            OperationKind::CheckLocal { name, .. } => {
                if !self.frame.locals.contains_key(name) {
                    return Err(ManagedFailure(Failure::new(self.frame.failure.diagnostic(
                        FailureCause::Error(rinzu_runtime::failure::ErrorKind::UnboundValue),
                        operation.range,
                    )))
                    .into());
                }
                return Ok(Step::none());
            }
            OperationKind::EnterErrorGroup { scope } => {
                self.frame.failure.enter_error_group(*scope);
                return Ok(Step::none());
            }
            OperationKind::FinishErrorGroup { scope, handles } => {
                let errors = self.frame.failure.finish_error_group(*scope, *handles)?;
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .caught_errors
                    .extend(errors.iter().map(Failure::diagnostics_only));
                self.frame
                    .data_roots
                    .as_ref()
                    .ok_or_else(|| anyhow!("Error group requires Owner"))?
                    .owner
                    .borrow_mut()
                    .error_list(&errors)?
            }
            OperationKind::RecordTaskError { scope } => {
                self.frame.failure.record_error(
                    *scope,
                    self.frame.error_origin.take(),
                    self.frame.block,
                )?;
                return Ok(Step::none());
            }
            OperationKind::TakeTaskErrors { scope } => {
                let body = self.task_groups.last().unwrap().body;
                let errors = self
                    .shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .scope(body)
                    .errors();
                if self
                    .frame
                    .failure
                    .cleanup_incoming()
                    .is_some_and(|f| !f.recoverable())
                {
                    self.frame
                        .data_roots
                        .as_ref()
                        .unwrap()
                        .owner
                        .borrow_mut()
                        .error_list(&[])?
                } else {
                    self.frame.failure.settle_task_join(*scope, None)?;
                    self.shared
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .caught_errors
                        .extend(errors.clone());
                    self.frame
                        .data_roots
                        .as_ref()
                        .unwrap()
                        .owner
                        .borrow_mut()
                        .error_list(&errors)?
                }
            }
            OperationKind::EnterTaskScope { scope, all, .. } => {
                let (body, cleanup) = {
                    let mut shared = self
                        .shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        shared.task_group(self.scope, *all)?,
                        shared.task_group(self.scope, *all)?,
                    )
                };
                self.task_groups.push(TaskGroup {
                    source: *scope,
                    body,
                    cleanup,
                    cleaning: false,
                    allow_cancel: !self.frame.failure.in_cleanup() && !self.cancellation.masked,
                });
                return Ok(Step::none());
            }
            OperationKind::ErrorGroupOutcome { cleanup, .. }
            | OperationKind::TaskScopeOutcome { cleanup, .. } => {
                if let Some(failure) = self.frame.failure.finish_task_scope(*cleanup)? {
                    return Err(ManagedFailure(failure).into());
                }
                return Ok(Step::none());
            }
            OperationKind::JoinTaskScope { scope, cleanup } => {
                return Ok(Step {
                    value: None,
                    action: Some(self.begin_group_join(*scope, *cleanup)?),
                });
            }
            OperationKind::StaticHandlerReference { target } => Value::Handler(target.clone()),
            OperationKind::StoreLocal { name, value } => {
                let value = self.value(*value)?.clone();
                self.frame.locals.insert(name.clone(), value);
                Value::Unit
            }
            OperationKind::Unary {
                operator, operand, ..
            } => unary(*operator, self.value(*operand)?).map_err(|_| {
                ManagedFailure(Failure::new(
                    self.frame
                        .failure
                        .diagnostic(FailureCause::ArithmeticTrap, operation.range),
                ))
            })?,
            OperationKind::Binary {
                operator,
                left,
                right,
                ..
            } => binary(*operator, self.value(*left)?, self.value(*right)?).map_err(|_| {
                ManagedFailure(Failure::new(
                    self.frame
                        .failure
                        .diagnostic(FailureCause::ArithmeticTrap, operation.range),
                ))
            })?,
            OperationKind::ManagedCall {
                target, arguments, ..
            } => {
                let arguments = self.arguments(arguments)?;
                let mut observed = Vec::new();
                let result = evaluate_main_call(
                    &self.module,
                    target,
                    arguments,
                    &self.cancellation,
                    &mut observed,
                    MainEvaluation {
                        task: &self.main_context(),
                    },
                );
                self.shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .caught_errors
                    .extend(observed);
                result?
            }
            OperationKind::Spawn {
                target, arguments, ..
            } => {
                let arguments = self.arguments(arguments)?;
                let prepared = PreparedSpawn::new(
                    &self.module,
                    target,
                    &arguments,
                    &self.admission,
                    &self.frame.data_roots.as_ref().expect("Main Owner").owner,
                );
                let future = self
                    .shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .spawn(
                        self.child_scope(),
                        target.clone(),
                        prepared,
                        self.frame
                            .failure
                            .diagnostic(FailureCause::Overloaded, operation.range)
                            .source,
                        false,
                    )?;
                self.start_child(future)?
            }
            OperationKind::NativeCall {
                target, arguments, ..
            } => {
                let arguments = self
                    .arguments(arguments)?
                    .iter()
                    .map(|value| self.frame.export(value))
                    .collect::<Result<Vec<_>>>()?;
                if let Some(binding) = self.native_async.get(target) {
                    let request = match binding {
                        NativeAsyncOperation::FileReadSize { path_argument } => {
                            let Some(Value::String(path)) = arguments.get(*path_argument) else {
                                bail!("{target} path argument is not a String")
                            };
                            PendingRequest::FileReadSize {
                                path: path.clone(),
                                source: self
                                    .frame
                                    .failure
                                    .diagnostic(FailureCause::Error(ErrorKind::Io), operation.range)
                                    .source,
                            }
                        }
                    };
                    let future = self
                        .shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .future(self.child_scope(), request)?;
                    self.start_child(future)?
                } else {
                    let packages = self.native_packages.as_ref().ok_or_else(|| {
                        anyhow!("native package binding is unavailable for {target}")
                    })?;
                    let import_key = packages.import_key(target).ok_or_else(|| {
                        anyhow!("native package export is unavailable for {target}")
                    })?;
                    let arguments = arguments
                        .iter()
                        .map(|value| native_scalar(&self.module, value))
                        .collect::<Result<Vec<_>>>()?;
                    let mut callback = SourceCallbackContext {
                        scope: self.scope,
                        module: Arc::clone(&self.module),
                        shared: Arc::clone(&self.shared),
                        owner: Rc::clone(
                            &self
                                .frame
                                .data_roots
                                .as_ref()
                                .ok_or_else(|| {
                                    anyhow!("native callback requires its caller Owner")
                                })?
                                .owner,
                        ),
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        source_native: Rc::clone(&self.source_native),
                    };
                    let invocation = NativeInvocationContextV1 {
                        struct_size: size_of::<NativeInvocationContextV1>() as u32,
                        abi_version: ABI_VERSION,
                        runtime_context: std::ptr::from_mut(&mut callback).cast(),
                        invoke_handler: Some(invoke_source_handler),
                    };
                    self.shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .events
                        .push(SourceAsyncEvent::NativeEntered {
                            scope: self.scope,
                            export: target.clone(),
                        });
                    let result = packages.invoke(import_key, &invocation, &arguments)?;
                    self.shared
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .events
                        .push(SourceAsyncEvent::NativeLeft {
                            scope: self.scope,
                            export: target.clone(),
                        });
                    let ty = &operation.result.as_ref().unwrap().ty;
                    if *ty == rinzu_compiler::Type::Unit {
                        Value::Unit
                    } else {
                        let kind = crate::scalar_backend::kind(
                            ty.scalar()
                                .ok_or_else(|| anyhow!("native v1 result is not scalar"))?,
                        );
                        Value::Scalar(ScalarValue::from_bits(kind, result as u64).map_err(
                            |error| anyhow!("invalid native v1 scalar result: {error:?}"),
                        )?)
                    }
                }
            }
            OperationKind::Await { future, .. } => {
                let Value::Future(future) = self.value(*future)? else {
                    bail!("await operand is not a Future")
                };
                return self.await_future(operation, *future);
            }
            OperationKind::StoreField { .. }
            | OperationKind::Field { .. }
            | OperationKind::ListElement { .. }
            | OperationKind::EnumPayload { .. } => {
                bail!("typed data projection requires the managed composite backend")
            }
            OperationKind::Construct { target, .. } => {
                bail!("baseline async cannot construct {target}")
            }
        };
        Ok(Step::value(value))
    }

    fn value(&self, value: ValueId) -> Result<&Value> {
        self.frame
            .values
            .get(&value)
            .ok_or_else(|| anyhow!("IR value {:?} is unavailable", value))
    }

    fn arguments(&self, values: &[ValueId]) -> Result<Vec<Value>> {
        values
            .iter()
            .map(|value| self.value(*value).cloned())
            .collect()
    }

    fn jump(&mut self, block: BlockId) {
        self.frame.jump(block);
    }
}

struct Step {
    value: Option<Value>,
    action: Option<TaskAction>,
}

impl Step {
    fn none() -> Self {
        Self {
            value: None,
            action: None,
        }
    }

    fn value(value: Value) -> Self {
        Self {
            value: Some(value),
            action: None,
        }
    }

    fn action(action: TaskAction) -> Self {
        Self {
            value: None,
            action: Some(action),
        }
    }
}

enum ActiveOperation {
    NativeCompletion {
        future: u64,
        result: ReactorOperation,
        transfer: bool,
        source: Option<SourcePosition>,
    },
    File {
        future: u64,
        result: FileReadResult,
        source: Option<SourcePosition>,
    },
    Timer {
        future: u64,
        result: ReactorOperation,
        source: Option<SourcePosition>,
    },
    Worker {
        future: u64,
        result: WorkerResult,
        descriptor: TypedDescriptor,
        result_type: rinzu_compiler::Type,
        value: TypedWorkerResult,
        source: Option<SourcePosition>,
    },
}

struct SourceOperations {
    deferred_native: std::collections::BTreeSet<u64>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    source_bindings: Arc<NativeBindings>,
    module: Arc<IrModule>,
    shared: Arc<Mutex<SharedState>>,
    owner: Rc<RefCell<OwnerValues>>,
    package_root: PathBuf,
    completion_timeout: Duration,
    cancel_worker_after_polls: Option<usize>,
    shutdown_worker_after_polls: Option<usize>,
    cancel_first_spawn: bool,
    cancelled_spawn: bool,
    mailbox: OperationMailbox,
    reactor: AsyncReactor,
    workers: CpuWorkerPool,
    active: BTreeMap<u64, (OperationId, ActiveOperation)>,
    winners: HashMap<OperationId, Resolution>,
    worker_completions: HashMap<OperationId, WorkerCompletion>,
    reactor_completions: HashMap<OperationId, ReactorCompletion>,
    cancelled_children: RefCell<std::collections::BTreeSet<u64>>,
    completion_signal: Arc<CompletionSignal>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    received_epoch: usize,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    readiness: BTreeMap<u64, rinzu_runtime::ready_work::ReadyReceipt>,
}

/// Helpers publish only native completion progress. The Owner snapshots before
/// draining both packet queues, then sleeps only if no later wake was published.
#[derive(Debug, Default)]
struct CompletionSignal {
    epoch: std::sync::atomic::AtomicUsize,
    gate: Mutex<()>,
    changed: std::sync::Condvar,
}
impl CompletionSignal {
    fn epoch(&self) -> usize {
        self.epoch.load(std::sync::atomic::Ordering::Acquire)
    }
    fn publish(&self) {
        let _gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        if self.epoch.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == usize::MAX {
            rinzu_runtime::allocation::fatal();
        }
        self.changed.notify_one();
    }
    fn wait(&self, observed: usize, interval: Duration) {
        let gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        if self.epoch() == observed {
            let _ = self
                .changed
                .wait_timeout(gate, interval)
                .unwrap_or_else(|p| p.into_inner());
        }
    }
}

impl SourceOperations {
    fn new(
        module: Arc<IrModule>,
        shared: Arc<Mutex<SharedState>>,
        options: &SourceAsyncOptions,
        owner: &Rc<RefCell<OwnerValues>>,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))] source_bindings: Arc<
            NativeBindings,
        >,
    ) -> Self {
        let completion_signal = Arc::new(CompletionSignal::default());
        let wake = {
            let signal = Arc::clone(&completion_signal);
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let producer = owner
                .borrow()
                .dispatcher
                .as_ref()
                .map(|dispatcher| dispatcher.queue().producer());
            Arc::new(move || {
                signal.publish();
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some(producer) = &producer {
                    producer.wake();
                }
            }) as Arc<dyn Fn() + Send + Sync>
        };
        Self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            source_bindings,
            module,
            shared,
            owner: Rc::clone(owner),
            package_root: options.package_root.clone(),
            completion_timeout: options.completion_timeout,
            cancel_worker_after_polls: options.cancel_worker_after_polls,
            shutdown_worker_after_polls: options.shutdown_worker_after_polls,
            cancel_first_spawn: options.cancel_first_spawn,
            cancelled_spawn: false,
            mailbox: OperationMailbox::new(
                StandaloneOptions::default().resources,
                Arc::clone(&wake),
            ),
            reactor: AsyncReactor::with_completion_wake(Arc::clone(&wake)),
            workers: CpuWorkerPool::with_completion_wake(wake),
            active: BTreeMap::new(),
            winners: HashMap::new(),
            worker_completions: HashMap::new(),
            reactor_completions: HashMap::new(),
            cancelled_children: RefCell::new(std::collections::BTreeSet::new()),
            deferred_native: Default::default(),
            completion_signal,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            received_epoch: 0,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            readiness: BTreeMap::new(),
        }
    }

    fn import_winners(&mut self) -> Result<()> {
        for event in self.mailbox.take_resolutions()? {
            ensure!(
                self.winners
                    .insert(event.operation, event.resolution)
                    .is_none(),
                "duplicate child winner"
            );
        }
        Ok(())
    }

    fn start(&mut self, future: u64) -> Result<()> {
        let (scope, request) = {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if shared.completed.contains_key(&future) {
                return Ok(());
            }
            (
                *shared
                    .future_scopes
                    .get(&future)
                    .ok_or_else(|| anyhow!("child has no parent scope"))?,
                shared
                    .pending
                    .remove(&future)
                    .ok_or_else(|| anyhow!("missing unstarted child {future}"))?,
            )
        };
        let source = match &request {
            PendingRequest::Spawn { source, .. }
            | PendingRequest::FileReadSize { source, .. }
            | PendingRequest::Timer { source, .. } => *source,
            PendingRequest::NativeCompletion { source, .. } => *source,
        };
        let path = match &request {
            PendingRequest::FileReadSize { path, .. } => {
                Some(package_path(&self.package_root, path)?)
            }
            _ => None,
        };
        #[allow(unused_mut)]
        let mut ticket = match self.mailbox.admit(
            TaskId(scope.ok_or_else(|| anyhow!("child started after task group settlement"))?),
            format!("source-future:{future}"),
        ) {
            Ok(ticket) => ticket,
            Err(StandaloneError::Resource(_)) => {
                if matches!(request, PendingRequest::NativeCompletion { .. }) {
                    // Its SDK operation is live: defer the pinned receipt until
                    // credit or physical completion, rather than lose its drain.
                    self.deferred_native.insert(future);
                    self.shared
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .pending
                        .insert(future, request);
                    return Ok(());
                }
                drop(request);
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .withdraw_admission(future)?;
                return Err(ManagedFailure(Failure::new(Diagnostic {
                    cause: FailureCause::Overloaded,
                    source,
                }))
                .into());
            }
            Err(error) => return Err(error.into()),
        };
        let operation_id = ticket.operation;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let symbol = match &request {
                PendingRequest::Spawn { target, .. } => target.as_str(),
                PendingRequest::FileReadSize { .. } => "reactor.file-read-size",
                PendingRequest::Timer { .. } => "reactor.timer",
                PendingRequest::NativeCompletion { transfer, .. } => {
                    if *transfer {
                        "reactor.native-transfer"
                    } else {
                        "reactor.native-completion"
                    }
                }
            };
            let receipt = self.owner.borrow().prepare_readiness(symbol, source);
            ticket.set_readiness(receipt.publication());
            if self.readiness.insert(future, receipt).is_some() {
                data::native_fatal(86);
            }
        }
        let operation = match request {
            PendingRequest::NativeCompletion {
                completion,
                transfer,
                source,
            } => {
                let result = if transfer {
                    self.reactor.submit_native_transfer(ticket, completion)?
                } else {
                    self.reactor.submit_native_completion(ticket, completion)?
                };
                ActiveOperation::NativeCompletion {
                    future,
                    result,
                    transfer,
                    source,
                }
            }
            PendingRequest::FileReadSize { source, .. } => {
                let result = match self.reactor.submit_file_read(ticket, path.unwrap()) {
                    Ok(result) => result,
                    Err(error) => {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        self.readiness.remove(&future).unwrap().receive();
                        self.shared
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .withdraw_admission(future)?;
                        return Err(error).context("start source file service");
                    }
                };
                ActiveOperation::File {
                    future,
                    source,
                    result,
                }
            }
            PendingRequest::Timer {
                milliseconds,
                source,
            } => {
                let duration = Duration::from_millis(u64::try_from(milliseconds)?);
                let result = match self.reactor.submit_timer(ticket, duration) {
                    Ok(result) => result,
                    Err(error) => {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        self.readiness.remove(&future).unwrap().receive();
                        self.shared
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .withdraw_admission(future)?;
                        return Err(error).context("start source timer service");
                    }
                };
                ActiveOperation::Timer {
                    future,
                    source,
                    result,
                }
            }
            PendingRequest::Spawn {
                target,
                prepared,
                source,
            } => {
                let PreparedSpawn {
                    descriptor,
                    result_descriptor,
                    input,
                    mut admitted,
                } = *prepared;
                let worker_descriptor = result_descriptor.clone();
                let function = &self.module.functions[function_index(&self.module, &target)?];
                let parameter_types = function.parameter_types.clone();
                let result_type = function.result_type.clone();
                let worker_result_type = result_type.clone();
                let value: TypedWorkerResult = Arc::new(Mutex::new(None));
                let worker_value = Arc::clone(&value);
                let module = Arc::clone(&self.module);
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let source_bindings = Arc::clone(&self.source_bindings);
                let cancel_worker_after_polls = self.cancel_worker_after_polls;
                let shutdown_worker_after_polls = self.shutdown_worker_after_polls;
                let cancel_first = self.cancel_first_spawn && !self.cancelled_spawn;
                if cancel_first {
                    self.cancelled_spawn = true;
                    self.cancelled_children.borrow_mut().insert(future);
                    ticket.cancel();
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                let native_ticket = ticket.clone();
                let stdout = self.owner.borrow().stdout.clone();
                let gc_stress = self.owner.borrow().gc_stress();
                let trace_enabled = self.owner.borrow().tracer.enabled();
                let result = self.workers.spawn(ticket, move |domain| {
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let source_native =
                        NativeEngine::new(source_bindings).map_err(|error| error.to_string())?;
                    let owner =
                        OwnerValues::worker(&module, domain).map_err(|error| error.to_string())?;
                    owner.borrow_mut().stdout = stdout;
                    owner.borrow_mut().tracer =
                        rinzu_runtime::error_trace::TraceContext::new(trace_enabled);
                    if gc_stress {
                        owner.borrow_mut().enable_gc_stress();
                    }
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let _native_registration = native_ticket
                        .observe_native_cancellation(owner.borrow().diagnostic_monitor().clone());
                    let packet = (|| {
                        let arguments = admitted
                            .prepare_typed_input(&descriptor, input, |values| {
                                owner
                                    .borrow_mut()
                                    .transport_materialize(&parameter_types, &values)
                                    .map_err(|_| {
                                        TransportError::TypedValue(
                                            rinzu_runtime::value::ValueError::TypeMismatch,
                                        )
                                    })
                            })
                            .map_err(|error| error.to_string())?;
                        let mut caught = Vec::new();
                        let outcome = evaluate_sync_context(
                            &module,
                            &target,
                            arguments,
                            Some(OwnerEvaluation {
                                domain: Some(domain),
                                callback: false,
                                owner: &owner,
                                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                                source_native: &source_native,
                            }),
                            &CancellationState {
                                shutdown_after_polls: shutdown_worker_after_polls,
                                ..CancellationState::new(cancel_worker_after_polls)
                            },
                            &mut caught,
                            None,
                        );
                        let outcome = match outcome {
                            Ok(value) => Ok(owner
                                .borrow_mut()
                                .transport_snapshot(
                                    std::slice::from_ref(&worker_result_type),
                                    &[value],
                                )
                                .map_err(|error| error.to_string())?),
                            Err(error) => {
                                let Some(failure) = error.downcast_ref::<ManagedFailure>() else {
                                    return Err(format!("{error:#}"));
                                };
                                Err(failure.0.clone())
                            }
                        };
                        encode_worker_packet(admitted, &worker_descriptor, outcome, &caught)
                    })();
                    let heap = owner
                        .borrow_mut()
                        .finish_worker(domain)
                        .map_err(|error| error.to_string())?;
                    let mut packet = packet?;
                    packet.heap = Some(SourceWorkerHeapEvidence {
                        domain: domain.id().0,
                        heap,
                    });
                    *worker_value
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(packet);
                    Ok(Vec::new())
                });
                let result = match result {
                    Ok(result) => result,
                    Err(_) => {
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        self.readiness.remove(&future).unwrap().receive();
                        // Rejected submission drops the entire unstarted closure
                        // before it publishes the private ticket's completion.
                        self.import_winners()?;
                        self.winners
                            .remove(&operation_id)
                            .ok_or_else(|| anyhow!("rejected child has no winner"))?;
                        self.mailbox.settle(operation_id)?;
                        self.shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .complete(
                                future,
                                Err(Failure::new(Diagnostic {
                                    cause: FailureCause::Overloaded,
                                    source,
                                })),
                            )?;
                        return Ok(());
                    }
                };
                if cancel_first {
                    result.request_cancel();
                }
                ActiveOperation::Worker {
                    future,
                    result,
                    descriptor: result_descriptor,
                    result_type,
                    value,
                    source,
                }
            }
        };
        let name = match &operation {
            ActiveOperation::NativeCompletion { transfer, .. } => {
                if *transfer {
                    "native-transfer"
                } else {
                    "native-completion"
                }
            }
            ActiveOperation::File { .. } => "file-read-size",
            ActiveOperation::Timer { .. } => "timer",
            ActiveOperation::Worker { .. } => "spawn",
        };
        ensure!(
            self.active
                .insert(future, (operation_id, operation))
                .is_none(),
            "duplicate child start"
        );
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .push(SourceAsyncEvent::Submitted {
                future,
                operation: name.to_owned(),
            });
        Ok(())
    }

    fn request_control(&self, future: u64, timed: bool) -> bool {
        use rinzu_runtime::scheduler::ResolveResult;
        if self.deferred_native.contains(&future) {
            let shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(PendingRequest::NativeCompletion { completion, .. }) =
                shared.pending.get(&future)
            {
                if completion.is_done() {
                    return false;
                }
                completion.cancel();
                self.cancelled_children.borrow_mut().insert(future);
                return true;
            }
        }
        let result = match self.active.get(&future) {
            Some((_, ActiveOperation::File { result, .. })) => {
                if timed {
                    result.request_timeout()
                } else {
                    result.request_cancel()
                }
            }
            Some((
                _,
                ActiveOperation::Timer { result, .. }
                | ActiveOperation::NativeCompletion { result, .. },
            )) => {
                if timed {
                    result.request_timeout()
                } else {
                    result.request_cancel()
                }
            }
            Some((_, ActiveOperation::Worker { result, .. })) => {
                if timed {
                    result.request_timeout()
                } else {
                    result.request_cancel()
                }
            }
            None => return false,
        };
        if result == ResolveResult::Won {
            self.cancelled_children.borrow_mut().insert(future);
            true
        } else {
            false
        }
    }
    fn cancel(&self, future: u64) -> bool {
        if self.active.contains_key(&future) || self.deferred_native.contains(&future) {
            let _ = self.request_control(future, false);
            true
        } else {
            false
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn has_ready_completion(&self) -> bool {
        self.completion_signal.epoch() != self.received_epoch
            || !self.reactor_completions.is_empty()
            || !self.worker_completions.is_empty()
    }

    fn receive_one_ready(&mut self) -> Result<bool> {
        let deferred = self.deferred_native.iter().copied().collect::<Vec<_>>();
        for future in deferred {
            let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
            let Some(PendingRequest::NativeCompletion {
                completion,
                transfer,
                source,
            }) = shared.pending.get(&future)
            else {
                bail!("missing native drain receipt")
            };
            let source = *source;
            if completion.is_done() {
                let value = if *transfer {
                    Value::int(completion.transfer_result())
                } else {
                    Value::Unit
                };
                let cancelled = self.cancelled_children.borrow().contains(&future);
                shared.pending.remove(&future);
                shared.complete(
                    future,
                    if cancelled {
                        Err(Failure::new(Diagnostic {
                            cause: FailureCause::Cancelled,
                            source,
                        }))
                    } else {
                        Ok(value)
                    },
                )?;
                self.deferred_native.remove(&future);
                return Ok(true);
            }
            drop(shared);
            if self.mailbox.resources().used.pending_operations
                < self.mailbox.resources().limits.pending_operations
            {
                self.start(future)?;
                if self.active.contains_key(&future) {
                    self.deferred_native.remove(&future);
                    if self.cancelled_children.borrow().contains(&future) {
                        self.request_control(future, false);
                    }
                }
            }
        }
        // Snapshot BEFORE reception. A later publication/wake keeps a newer
        // epoch and cannot be erased by this turn's empty snapshot.
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let epoch = self.completion_signal.epoch();
        while let Some(completion) = self.reactor.try_completion()? {
            ensure!(
                self.reactor_completions
                    .insert(completion.operation, completion)
                    .is_none(),
                "duplicate reactor completion"
            );
        }
        while let Some(completion) = self.workers.try_completion()? {
            ensure!(
                self.worker_completions
                    .insert(completion.operation, completion)
                    .is_none(),
                "duplicate Worker completion"
            );
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.received_epoch = epoch;
        }
        self.import_winners()?;
        let ready = self
            .active
            .iter()
            .find_map(|(future, (operation, active))| {
                let available = match active {
                    ActiveOperation::File { .. }
                    | ActiveOperation::Timer { .. }
                    | ActiveOperation::NativeCompletion { .. } => {
                        self.reactor_completions.contains_key(operation)
                    }
                    ActiveOperation::Worker { .. } => {
                        self.worker_completions.contains_key(operation)
                    }
                };
                available.then_some(*future)
            });
        if let Some(future) = ready {
            self.receive(future)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn receive_for_progress(&mut self, wanted: u64) -> Result<()> {
        ensure!(
            self.active.contains_key(&wanted) || self.deferred_native.contains(&wanted),
            "child {wanted} is not running"
        );
        let observed = self.completion_signal.epoch();
        if !self.receive_one_ready()? {
            self.completion_signal
                .wait(observed, self.completion_timeout);
        }
        Ok(())
    }

    fn receive(&mut self, future: u64) -> Result<()> {
        let (operation_id, active) = self
            .active
            .get(&future)
            .ok_or_else(|| anyhow!("child {future} is not running"))?;
        let operation_id = *operation_id;
        match active {
            ActiveOperation::File { .. }
            | ActiveOperation::Timer { .. }
            | ActiveOperation::NativeCompletion { .. } => {
                while !self.reactor_completions.contains_key(&operation_id) {
                    let completion = match self.reactor.wait_completion(self.completion_timeout) {
                        Ok(completion) => completion,
                        Err(ReactorError::CompletionTimeout) => continue,
                        Err(error) => return Err(error).context("wait for source reactor service"),
                    };
                    ensure!(
                        self.reactor_completions
                            .insert(completion.operation, completion)
                            .is_none(),
                        "duplicate reactor completion"
                    );
                }
            }
            ActiveOperation::Worker { .. } => {
                while !self.worker_completions.contains_key(&operation_id) {
                    let completion = match self.workers.wait_completion(self.completion_timeout) {
                        Ok(completion) => completion,
                        Err(WorkerWaitError::Timeout) => continue,
                        Err(error) => return Err(error).context("wait for source Worker"),
                    };
                    ensure!(
                        self.worker_completions
                            .insert(completion.operation, completion)
                            .is_none(),
                        "duplicate Worker completion"
                    );
                }
            }
        }
        self.import_winners()?;
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.readiness
                .remove(&future)
                .unwrap_or_else(|| data::native_fatal(86))
                .receive();
            self.owner.borrow().note_owner_turn();
        }
        let resolution = self
            .winners
            .remove(&operation_id)
            .ok_or_else(|| anyhow!("child has no terminal winner"))?;
        let (_, operation) = self.active.remove(&future).unwrap();
        match operation {
            ActiveOperation::NativeCompletion {
                future,
                source,
                result,
                transfer,
            } => {
                let value = if transfer {
                    Value::int(
                        result
                            .native_transfer_result()
                            .ok_or_else(|| anyhow!("missing scalar native receipt"))?,
                    )
                } else {
                    Value::Unit
                };
                self.reactor_completions.remove(&operation_id).unwrap();
                self.shared
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .complete(future, receiver_outcome(Ok(value), &resolution, source))?;
            }
            ActiveOperation::Timer { future, source, .. } => {
                self.reactor_completions.remove(&operation_id).unwrap();
                let mut shared = self.shared.lock().unwrap_or_else(|p| p.into_inner());
                shared
                    .events
                    .push(SourceAsyncEvent::TimerCompleted { future });
                shared.complete(
                    future,
                    receiver_outcome(Ok(Value::Unit), &resolution, source),
                )?;
            }
            ActiveOperation::File {
                future,
                result,
                source,
            } => {
                self.reactor_completions.remove(&operation_id).unwrap();
                let bytes = result
                    .take()
                    .ok_or_else(|| anyhow!("file result {future} is missing"))?;
                let mut shared = self
                    .shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let outcome = match bytes {
                    Ok(bytes) => {
                        let size = i64::try_from(bytes.len())?;
                        shared.events.push(SourceAsyncEvent::FileCompleted {
                            future,
                            bytes: bytes.len(),
                        });
                        Ok(Value::int(size))
                    }
                    Err(_) => Err(Failure::new(Diagnostic {
                        cause: FailureCause::Error(ErrorKind::Io),
                        source,
                    })),
                };
                shared.complete(future, receiver_outcome(outcome, &resolution, source))?;
            }
            ActiveOperation::Worker {
                future,
                result,
                descriptor,
                result_type,
                value,
                source,
            } => {
                let completion = self.worker_completions.remove(&operation_id).unwrap();
                let output = result
                    .take()
                    .ok_or_else(|| anyhow!("worker result {future} is missing"))?;
                let mut shared = self
                    .shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                shared.worker_returns = shared
                    .worker_returns
                    .checked_add(1)
                    .expect("Worker return counter");
                let outcome = match output {
                    Ok(output) => {
                        let WorkerPacket {
                            outcome,
                            caught,
                            admitted,
                            heap,
                        } = value
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .take()
                            .ok_or_else(|| anyhow!("typed Worker result is missing"))?;
                        let failure_descriptor = failure_descriptor();
                        let result = match outcome {
                            WorkerOutcome::Returned(buffer) => {
                                let values = buffer.decode_typed(&descriptor)?;
                                let mut owner = self.owner.borrow_mut();
                                let value = owner
                                    .transport_materialize(
                                        std::slice::from_ref(&result_type),
                                        &values,
                                    )?
                                    .pop()
                                    .ok_or_else(|| anyhow!("Worker result is empty"))?;
                                if !matches!(
                                    resolution,
                                    Resolution::Cancelled | Resolution::TimedOut
                                ) {
                                    owner.retain_result(future, &value);
                                }
                                drop(buffer);
                                Ok(value)
                            }
                            WorkerOutcome::Failed(buffer) => Err(materialize_failure(
                                buffer.materialize(&failure_descriptor)?,
                            )?),
                            WorkerOutcome::TransportRejected => Err(Failure::new(Diagnostic {
                                cause: FailureCause::Overloaded,
                                source,
                            })),
                        };
                        for packet in caught {
                            shared.caught_errors.push(materialize_failure(
                                packet.materialize(&failure_descriptor)?,
                            )?);
                        }
                        drop(admitted);
                        if let Some(heap) = heap {
                            shared.worker_heaps.push(heap);
                        }
                        shared.events.push(SourceAsyncEvent::WorkerCompleted {
                            future,
                            domain: completion.domain.0,
                            worker_thread: output.evidence.thread,
                        });
                        result
                    }
                    Err(error) => Err(Failure::new(Diagnostic {
                        cause: if error.contains("cancelled") {
                            FailureCause::Cancelled
                        } else {
                            FailureCause::RuntimePanic
                        },
                        source,
                    })),
                };
                shared.complete(future, receiver_outcome(outcome, &resolution, source))?;
            }
        }
        self.mailbox.settle(operation_id)?;
        Ok(())
    }

    fn is_baseline(&self) -> bool {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if !self.readiness.is_empty() {
            return false;
        }
        self.mailbox.resources().used.is_baseline()
            && self.active.is_empty()
            && self.winners.is_empty()
            && self.worker_completions.is_empty()
            && self.reactor_completions.is_empty()
    }
    fn finish_helpers(&mut self) {
        self.reactor.shutdown();
        self.workers.shutdown();
    }
}

impl Drop for SourceOperations {
    fn drop(&mut self) {
        // Internal execution errors still request cancellation and keep the
        // physical pool/service owners alive through their native returns.
        for future in self.active.keys() {
            self.cancel(*future);
        }
    }
}

enum WorkerOutcome {
    Returned(TransportBuffer),
    Failed(TransportBuffer),
    TransportRejected,
}

struct WorkerPacket {
    outcome: WorkerOutcome,
    caught: Vec<TransportBuffer>,
    admitted: AdmittedSpawn,
    heap: Option<SourceWorkerHeapEvidence>,
}

type TypedWorkerResult = Arc<Mutex<Option<WorkerPacket>>>;

fn encode_worker_packet(
    admitted: AdmittedSpawn,
    descriptor: &TypedDescriptor,
    outcome: std::result::Result<Vec<rinzu_runtime::value::Value>, Failure>,
    caught: &[Failure],
) -> std::result::Result<WorkerPacket, String> {
    let failure_descriptor = failure_descriptor();
    let encoded = (|| -> std::result::Result<_, SpawnAdmissionError> {
        let outcome = match outcome {
            Ok(value) => WorkerOutcome::Returned(admitted.encode_typed_result(descriptor, &value)?),
            Err(failure) => WorkerOutcome::Failed(
                admitted.encode_result(&failure_descriptor, &failure_value(&failure))?,
            ),
        };
        let caught = caught
            .iter()
            .map(|failure| admitted.encode_result(&failure_descriptor, &failure_value(failure)))
            .collect::<std::result::Result<_, _>>()?;
        Ok((outcome, caught))
    })();
    let (outcome, caught) = match encoded {
        Ok(encoded) => encoded,
        Err(SpawnAdmissionError::Transport(
            TransportError::OperationLimit { .. } | TransportError::ProcessLimit { .. },
        )) => (WorkerOutcome::TransportRejected, Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    Ok(WorkerPacket {
        outcome,
        caught,
        admitted,
        heap: None,
    })
}

fn receiver_outcome(
    outcome: std::result::Result<Value, Failure>,
    resolution: &Resolution,
    source: Option<SourcePosition>,
) -> std::result::Result<Value, Failure> {
    if !matches!(resolution, Resolution::Cancelled | Resolution::TimedOut) {
        return outcome;
    }
    let mut cancelled = Failure::new(Diagnostic {
        cause: FailureCause::Cancelled,
        source,
    });
    if let Err(failure) = outcome {
        if failure.primary.cause == FailureCause::Cancelled {
            return Err(failure);
        }
        cancelled.cleanup.push(failure.primary);
        cancelled.cleanup.extend(failure.cleanup);
    }
    Err(cancelled)
}

struct SourceCallbackContext {
    scope: u64,
    module: Arc<IrModule>,
    shared: Arc<Mutex<SharedState>>,
    owner: Rc<RefCell<OwnerValues>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    source_native: Rc<NativeEngine>,
}

unsafe extern "C" fn invoke_source_handler(context: *mut core::ffi::c_void, handler: u64) -> i64 {
    if context.is_null() {
        return 0;
    }
    let Ok(handler) = usize::try_from(handler) else {
        return 0;
    };
    // SAFETY: a synchronous native package call receives a pointer to the live
    // SourceCallbackContext stack value and cannot retain it past return.
    let context = unsafe { &*(context.cast::<SourceCallbackContext>()) };
    let Some(function) = context.module.functions.get(handler) else {
        return 0;
    };
    context
        .shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .events
        .push(SourceAsyncEvent::CallbackEntered {
            scope: context.scope,
            handler: function.name.clone(),
        });
    let mut observed = Vec::new();
    let result = evaluate_sync_observed(
        &context.module,
        &function.name,
        Vec::new(),
        Some(OwnerEvaluation {
            domain: None,
            callback: false,
            owner: &context.owner,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            source_native: &context.source_native,
        }),
        &mut observed,
    )
    .and_then(|value| value.as_i64())
    .unwrap_or(0);
    context
        .shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .caught_errors
        .extend(observed);
    context
        .shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .events
        .push(SourceAsyncEvent::CallbackLeft {
            scope: context.scope,
            handler: function.name.clone(),
        });
    result
}

fn native_scalar(module: &IrModule, value: &Value) -> Result<i64> {
    match value {
        Value::Unit => Ok(0),
        Value::Scalar(value) if matches!(value.kind(), ScalarKind::I64 | ScalarKind::Bool) => {
            Ok(value.bits() as i64)
        }
        Value::Handler(name) => i64::try_from(function_index(module, name)?)
            .context("static handler index does not fit native scalar"),
        value => bail!("native v1 scalar ABI does not support {value:?}"),
    }
}

pub fn run_source_async(
    module: &IrModule,
    entry: &str,
    options: SourceAsyncOptions,
) -> Result<SourceAsyncRun> {
    run_source_async_admitted(module, entry, options, SpawnAdmissionController::new())
}

/// Run with immutable failure and resource evidence even when the root fails.
pub fn run_source_async_report(
    module: &IrModule,
    entry: &str,
    options: SourceAsyncOptions,
) -> Result<SourceAsyncRun> {
    run_source_async_report_admitted(module, entry, options, SpawnAdmissionController::new())
}

fn run_source_async_admitted(
    module: &IrModule,
    entry: &str,
    options: SourceAsyncOptions,
    admission: SpawnAdmissionController,
) -> Result<SourceAsyncRun> {
    let run = run_source_async_report_admitted(module, entry, options, admission)?;
    if let Some(failure) = &run.failure_details
        && !run.shutdown_complete
    {
        return Err(ManagedFailure(failure.clone()).into());
    }
    Ok(run)
}

fn run_source_async_report_admitted(
    module: &IrModule,
    entry: &str,
    options: SourceAsyncOptions,
    admission: SpawnAdmissionController,
) -> Result<SourceAsyncRun> {
    crate::ensure_source_async_backend(module)?;
    if options.record_transport_evidence {
        admission.enable_transport_evidence();
    }
    let owner_thread = std::thread::current().id();
    let module = Arc::new(module.clone());
    let entry_index = function_index(&module, entry)?;
    let owner_values = OwnerValues::new(&module)?;
    if options.gc_stress {
        owner_values.borrow_mut().enable_gc_stress();
    }
    let stdout = rinzu_runtime::stdio::Output::new(options.capture_stdout);
    owner_values.borrow_mut().stdout = stdout.clone();
    owner_values.borrow_mut().tracer =
        rinzu_runtime::error_trace::TraceContext::from_environment(options.capture_error_trace);
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    if module
        .source_native_signatures
        .iter()
        .any(|signature| signature.is_supplant)
    {
        // SAFETY: this Main factory owns an independent runtime dispatcher;
        // the initial foreign-only consumer has no external hook/context.
        let dispatcher = Rc::new(unsafe {
            rinzu_runtime::owner_dispatcher::OwnerDispatcher::new(
                None,
                None,
                None,
                std::ptr::null_mut(),
            )
        });
        #[cfg(feature = "test-dispatcher-probe")]
        if let Some(count) = FOREIGN_RETURN_PROBE.with(Cell::take) {
            dispatcher.test_return_after_published(count);
        }
        owner_values.borrow_mut().dispatcher = Some(dispatcher);
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let source_bindings = Arc::new(NativeBindings::new(
        &module,
        options.source_packages.as_ref(),
    )?);
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let source_native = Rc::new(NativeEngine::new(Arc::clone(&source_bindings))?);
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let supplants = Rc::new(RefCell::new(native::Supplants::default()));
    let shared = Arc::new(Mutex::new(SharedState {
        trace_enabled: owner_values.borrow().tracer.enabled(),
        main_task_limit: options.main_task_limit,
        ..SharedState::default()
    }));
    owner_values.borrow_mut().future_state = Arc::downgrade(&shared);
    let continuations = Rc::new(RefCell::new(BTreeMap::new()));
    let operations = Rc::new(RefCell::new(SourceOperations::new(
        Arc::clone(&module),
        Arc::clone(&shared),
        &options,
        &owner_values,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Arc::clone(&source_bindings),
    )));
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    dispatcher::connect(&owner_values, &shared, &continuations, &operations);
    let cancellation = CancellationState {
        shutdown_after_polls: options.shutdown_after_polls,
        ..CancellationState::new(options.cancel_after_polls)
    };
    let root_cancellation = Rc::clone(&cancellation.requested);
    let task = SourceTask {
        module: Arc::clone(&module),
        native_async: options.native_async,
        frame: Frame::new(
            &module,
            entry_index,
            &module.functions[entry_index],
            Vec::new(),
        )
        .with_owner(&owner_values),
        shared: Arc::clone(&shared),
        owner_thread,
        native_packages: options.native_packages.map(Rc::new),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        source_native,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        supplants: Rc::downgrade(&supplants),
        cancellation,
        admission: admission.clone(),
        continuations: Rc::downgrade(&continuations),
        operations: Rc::downgrade(&operations),
        scope: 0,
        completion: None,
        closing: None,
        closing_child: None,
        closing_group: None,
        task_groups: Vec::new(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        readiness: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        wait_dependency: None,
    };
    let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
    let main_task = runtime
        .start_main(Box::new(task))
        .context("start source async main")?;
    let origin = OriginDomainId(0);
    let mut root_control_sent = false;

    loop {
        match runtime
            .run_until_stalled()
            .context("run source continuation")?
        {
            RunStatus::Complete(TaskOutcome::Returned) => break,
            RunStatus::Complete(TaskOutcome::Failed(_)) => break,
            RunStatus::Complete(TaskOutcome::Cancelled) => {
                shared
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .result
                    .get_or_insert_with(|| Err(Failure::new(FailureCause::Cancelled)));
                break;
            }
            RunStatus::ShutdownComplete(report) => {
                bail!("source async runtime shut down with {}", report.exit_code)
            }
            RunStatus::Stalled { .. } => {
                if !root_control_sent
                    && shared
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .shutdown_requested
                {
                    runtime.cancel_task(main_task)?;
                    root_control_sent = true;
                    continue;
                }
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native::Supplants::drive(&supplants)?;
                let tickets = runtime.take_operation_tickets();
                ensure!(
                    !tickets.is_empty(),
                    "runtime stalled without operation tickets"
                );
                for parent_ticket in tickets {
                    let parent_future = parent_ticket
                        .label
                        .strip_prefix("source-future:")
                        .ok_or_else(|| {
                            anyhow!("unexpected operation label {}", parent_ticket.label)
                        })?
                        .parse::<u64>()
                        .context("parse source future id")?;
                    loop {
                        resume_ready_continuations(&shared, &continuations)?;
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        if native::Supplants::drive(&supplants)? {
                            continue;
                        }
                        if shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .completed
                            .contains_key(&parent_future)
                        {
                            break;
                        }
                        let future = continuation_dependency(&continuations, parent_future)?;
                        operations.borrow_mut().receive_for_progress(future)?;
                    }
                    let cancelled = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).completed
                        .get(&parent_future).is_some_and(|outcome| matches!(outcome, Err(failure) if failure.primary.cause == FailureCause::Cancelled));
                    if cancelled {
                        parent_ticket.cancel();
                    } else {
                        parent_ticket.complete(Ok(()));
                    }
                }
            }
        }
    }

    let transport = admission.snapshot(origin);
    let resources_returned = runtime.resources().used.is_baseline()
        && operations.borrow().is_baseline()
        && continuations.borrow().is_empty()
        && transport.worker_domains == 0
        && transport.in_flight_for_origin == 0
        && transport.transport_bytes == 0;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let resources_returned = resources_returned && supplants.borrow().is_baseline();
    drop(runtime);
    operations.borrow_mut().finish_helpers();
    let heap = owner_values.borrow_mut().finish()?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let native_calls = {
        let mut evidence = source_bindings.evidence();
        if let Some(dispatcher) = &owner_values.borrow().dispatcher {
            let queue = dispatcher.queue().stats();
            evidence.foreign = ForeignInvocationEvidence {
                created: queue.created,
                published: queue.published,
                received: queue.received,
                retired: queue.retired,
                live: queue.live,
                bytes: queue.bytes,
                queued: queue.queued,
                wake_pins: queue.wake_pins,
            };
        }
        evidence
    };
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    let native_calls = SourceNativeCallEvidence::default();
    let mut shared = shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(error) = shared.internal_error.take() {
        bail!(error);
    }
    let outcome = shared
        .result
        .take()
        .ok_or_else(|| anyhow!("source async entry returned no value"))?;
    let (result, failure_details) = match outcome {
        Ok(value) => (value.as_i64()?, None),
        Err(failure) => (0, Some(failure)),
    };
    let last_awaited_result = shared
        .last_awaited
        .as_ref()
        .and_then(|value| value.as_i64().ok());
    let last_awaited_scalar = shared.last_awaited.as_ref().and_then(Value::scalar);
    let shutdown_complete = shared.shutdown_requested
        && failure_details
            .as_ref()
            .is_some_and(|f| f.primary.cause == FailureCause::Shutdown && f.cleanup.is_empty());
    let resources_returned = resources_returned
        && native_calls.is_baseline()
        && heap.objects == 0
        && heap.roots == 0
        && heap.userdata.is_baseline()
        && shared.worker_heaps.iter().all(|worker| {
            worker.heap.objects == 0 && worker.heap.roots == 0 && worker.heap.userdata.is_baseline()
        });
    Ok(SourceAsyncRun {
        exit_status: if (failure_details.is_none() || shutdown_complete) && resources_returned {
            shared.exit_status
        } else {
            1
        },
        future_admissions: std::mem::take(&mut shared.future_admissions),
        peak_future_records: shared.peak_future_records,
        gc_retired_futures: shared.gc_retired_futures,
        remaining_future_records: shared.future_scopes.len(),
        shutdown_complete,
        stdout: stdout.take(),
        result,
        failure_details,
        caught_errors: std::mem::take(&mut shared.caught_errors),
        source_files: crate::baseline::source_identities(&module).1,
        last_awaited_result,
        last_awaited_scalar,
        awaited_scalars: std::mem::take(&mut shared.awaited_scalars),
        awaited_tasks: std::mem::take(&mut shared.awaited_tasks),
        transport,
        transport_evidence: admission.transport_evidence(),
        cancelled_children: operations
            .borrow()
            .cancelled_children
            .borrow()
            .union(&shared.control_requests)
            .copied()
            .collect(),
        scope_cancellation_requests: shared.scope_cancellation_requests.iter().copied().collect(),
        root_cancellation_requested: root_cancellation.get(),
        control_attempts: shared.control_attempts.iter().copied().collect(),
        owner_thread,
        events: std::mem::take(&mut shared.events),
        resources_returned,
        heap,
        worker_heaps: std::mem::take(&mut shared.worker_heaps),
        worker_returns: shared.worker_returns,
        native_calls,
        native_process: crate::NativeProcessEvidence::capture(),
    })
}

fn package_path(root: &Path, path: &str) -> Result<PathBuf> {
    ensure!(
        !path.is_empty() && !path.as_bytes().contains(&0),
        "async package path must be nonempty and contain no NUL"
    );
    let path = Path::new(path);
    ensure!(!path.is_absolute(), "async package path must be relative");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "async package path cannot contain `.` or `..`"
    );
    Ok(root.join(path))
}

fn function_index(module: &IrModule, name: &str) -> Result<usize> {
    module
        .functions
        .iter()
        .position(|function| function.name == name)
        .ok_or_else(|| anyhow!("managed function {name} is unavailable"))
}

fn failure_message(failure: &Failure) -> String {
    let mut message = failure.primary.cause.message().to_owned();
    for diagnostic in &failure.cleanup {
        message.push_str("; cleanup: ");
        message.push_str(diagnostic.cause.message());
    }
    message
}

fn evaluate_sync_observed(
    module: &IrModule,
    name: &str,
    arguments: Vec<Value>,
    worker: Option<OwnerEvaluation<'_>>,
    observed: &mut Vec<Failure>,
) -> Result<Value> {
    evaluate_sync_context(
        module,
        name,
        arguments,
        worker,
        &CancellationState::default(),
        observed,
        None,
    )
}

#[derive(Clone, Copy)]
struct MainEvaluation<'a> {
    task: &'a MainTaskContext,
}

#[derive(Clone, Copy)]
struct OwnerEvaluation<'a> {
    domain: Option<&'a WorkerDomain>,
    /// A reentrant source-native callback publishes observations to its Owner
    /// until the outer physical native return. A standalone sync evaluation
    /// can also have no Worker domain, but uses its ordinary observed vector.
    callback: bool,
    owner: &'a Rc<RefCell<OwnerValues>>,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    source_native: &'a NativeEngine,
}

fn evaluate_main_call(
    module: &IrModule,
    name: &str,
    arguments: Vec<Value>,
    cancellation: &CancellationState,
    observed: &mut Vec<Failure>,
    main: MainEvaluation<'_>,
) -> Result<Value> {
    let index = function_index(module, name)?;
    if !module.functions[index].is_async {
        let mut inherited = cancellation.clone();
        inherited.masked |= main.task.cleanup;
        return evaluate_sync_context(
            module,
            name,
            arguments,
            None,
            &inherited,
            observed,
            Some(main),
        );
    }
    // Admission prepares and roots the environment, but never executes the
    // body. Initial execution uses the same runtime turns as later resumes.
    let future = {
        let mut shared = main.task.shared.lock().unwrap_or_else(|p| p.into_inner());
        if !main.task.cleanup
            && !cancellation.masked
            && shared
                .main_task_limit
                .is_some_and(|limit| shared.live_main_tasks.len() >= limit)
        {
            return Err(ManagedFailure(Failure::new(Diagnostic {
                cause: FailureCause::Overloaded,
                source: Some(SourcePosition {
                    file: module.functions[index].range.file_id.0 as u64,
                    start_byte: module.functions[index].range.start_byte as u32,
                    end_byte: module.functions[index].range.end_byte as u32,
                }),
            }))
            .into());
        }
        let future =
            shared.admit_with_lease(main.task.child_scope, main.task.callback_admission)?;
        shared.live_main_tasks.insert(future);
        future
    };
    let task = SourceTask {
        module: Arc::clone(&main.task.module),
        native_async: main.task.native_async.clone(),
        frame: Frame::new(module, index, &module.functions[index], arguments)
            .with_owner(&main.task.owner),
        shared: Arc::clone(&main.task.shared),
        owner_thread: main.task.owner_thread,
        native_packages: main.task.native_packages.clone(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        source_native: Rc::clone(&main.task.source_native),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        supplants: main.task.supplants.clone(),
        cancellation: {
            let mut child = cancellation.child();
            child.masked |= main.task.cleanup;
            child
        },
        admission: main.task.admission.clone(),
        continuations: main.task.continuations.clone(),
        operations: main.task.operations.clone(),
        scope: future + 1,
        completion: Some(future),
        closing: None,
        closing_child: None,
        closing_group: None,
        task_groups: Vec::new(),
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        readiness: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        wait_dependency: None,
    };
    let tasks = main
        .task
        .continuations
        .upgrade()
        .ok_or_else(|| anyhow!("continuation registry ended"))?;
    ensure!(
        tasks.borrow_mut().insert(future, task).is_none(),
        "duplicate queued task"
    );
    Ok(Value::Future(future))
}

fn park_continuation(
    tasks: &Rc<RefCell<BTreeMap<u64, SourceTask>>>,
    mut task: SourceTask,
    event: TaskEvent,
) -> Result<()> {
    let future = task
        .completion
        .ok_or_else(|| anyhow!("callee has no Future"))?;
    let action = task.resume(event);
    match action {
        TaskAction::Pending { .. } => {
            ensure!(
                task.waiting_future().is_some(),
                "callee suspended without a dependency"
            );
            ensure!(
                tasks.borrow_mut().insert(future, task).is_none(),
                "duplicate callee continuation"
            );
        }
        TaskAction::Return | TaskAction::Fail(_) | TaskAction::Cancelled => {
            let shared = task
                .shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            ensure!(
                shared.completed.contains_key(&future),
                "callee failed internally: {:?}",
                shared.internal_error
            );
        }
        _ => bail!("unexpected action from source callee"),
    }
    Ok(())
}

fn resume_ready_continuations(
    shared: &Arc<Mutex<SharedState>>,
    tasks: &Rc<RefCell<BTreeMap<u64, SourceTask>>>,
) -> Result<()> {
    loop {
        let ready = {
            let shared = shared
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            tasks
                .borrow()
                .iter()
                .filter_map(|(future, task)| task.ready_event(&shared).map(|_| *future))
                .collect::<Vec<_>>()
        };
        if ready.is_empty() {
            return Ok(());
        }
        for future in ready {
            // An earlier task can cancel/resume a later member of this ready
            // snapshot. Its old Start/Operation event is no longer valid.
            let (task, event) = {
                let shared = shared.lock().unwrap_or_else(|p| p.into_inner());
                let mut pending = tasks.borrow_mut();
                let Some(event) = pending
                    .get(&future)
                    .and_then(|task| task.ready_event(&shared))
                else {
                    continue;
                };
                let task = pending.remove(&future).expect("checked ready continuation");
                (task, event)
            };
            park_continuation(tasks, task, event)?;
        }
    }
}

fn continuation_dependency(
    tasks: &Rc<RefCell<BTreeMap<u64, SourceTask>>>,
    mut future: u64,
) -> Result<u64> {
    let mut visited = std::collections::HashSet::new();
    while let Some(task) = tasks.borrow().get(&future) {
        ensure!(visited.insert(future), "cyclic Future dependency");
        future = task
            .waiting_future()
            .ok_or_else(|| anyhow!("pending callee has no dependency"))?;
    }
    Ok(future)
}

fn record_sync_caught(
    failure: &Failure,
    worker: Option<OwnerEvaluation<'_>>,
    main: Option<MainEvaluation<'_>>,
    observed: &mut Vec<Failure>,
) {
    let diagnostic = failure.diagnostics_only();
    if let Some(main) = main {
        main.task
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .caught_errors
            .push(diagnostic);
    } else if let Some(callback) = worker.filter(|owner| owner.callback) {
        callback
            .owner
            .borrow_mut()
            .record_callback_error(diagnostic);
    } else {
        observed.push(diagnostic);
    }
}

fn evaluate_sync_context(
    module: &IrModule,
    name: &str,
    arguments: Vec<Value>,
    worker: Option<OwnerEvaluation<'_>>,
    cancellation: &CancellationState,
    observed: &mut Vec<Failure>,
    main: Option<MainEvaluation<'_>>,
) -> Result<Value> {
    let index = function_index(module, name)?;
    let function = &module.functions[index];
    let mut frame = Frame::new(module, index, function, arguments);
    if let Some(main) = main {
        frame = frame.with_owner(&main.task.owner);
    } else if let Some(worker) = worker {
        frame = frame.with_owner(worker.owner);
    }
    let _error_trace = frame.failure.tracer().enter(
        &function.name,
        frame
            .failure
            .diagnostic(FailureCause::RuntimePanic, function.range)
            .source,
    );
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let _activity = if worker.is_some() {
        frame.data_roots.as_ref().and_then(|roots| {
            let owner = roots.owner.borrow();
            if !owner.is_managed_activation() {
                return None;
            }
            let source = frame
                .failure
                .diagnostic(FailureCause::RuntimePanic, function.range)
                .source;
            let activity = owner.prepare_activity(
                rinzu_runtime::native_diagnostics::ActivityKind::Managed,
                &function.name,
                source,
            );
            activity.start();
            Some(activity)
        })
    } else {
        None
    };
    'blocks: loop {
        let block = function
            .blocks
            .iter()
            .find(|block| block.id == frame.block)
            .ok_or_else(|| anyhow!("missing IR block {:?}", frame.block))?;
        frame.failure.enter(block);
        for operation in block.operations.iter().skip(frame.operation) {
            frame.operation += 1;
            frame.failure.before(operation);
            let value = (|| -> Result<Value> {
                let resolved = resolve_indirect(operation, &frame.values)?;
                let operation = resolved.as_ref();
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                if let Some(engine) = main
                    .map(|main| main.task.source_native.as_ref())
                    .or_else(|| worker.map(|worker| worker.source_native))
                    && let Some(value) = frame.native_operation(
                        operation,
                        engine,
                        cancellation,
                        main.map(|main| main.task),
                    )?
                {
                    if !worker.is_some_and(|owner| owner.callback) {
                        let errors = frame
                            .data_roots
                            .as_ref()
                            .unwrap()
                            .owner
                            .borrow_mut()
                            .take_callback_errors();
                        if let Some(main) = main {
                            main.task
                                .shared
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .caught_errors
                                .extend(errors);
                        } else {
                            observed.extend(errors);
                        }
                    }
                    return Ok(value);
                }
                if let Some(value) = frame.data_operation(operation)? {
                    return Ok(value);
                }
                let value = match &operation.kind {
                    OperationKind::CoreLength {
                        operand,
                        kind: rinzu_compiler::CoreLengthKind::Utf8,
                    } => utf8_length(
                        frame
                            .values
                            .get(operand)
                            .ok_or_else(|| anyhow!("String operand is missing"))?,
                    )?,
                    OperationKind::ErrorMessage { error } => {
                        let Value::Error(failure) = frame
                            .values
                            .get(error)
                            .ok_or_else(|| anyhow!("Error operand is missing"))?
                        else {
                            bail!("Error.message operand is not Error")
                        };
                        Value::String(failure_message(failure))
                    }
                    OperationKind::Stdout { .. }
                    | OperationKind::FormatString { .. }
                    | OperationKind::CoreLength { .. }
                    | OperationKind::DataOperation { .. }
                    | OperationKind::BytesPad { .. }
                    | OperationKind::ErrorIs { .. }
                    | OperationKind::ErrorCast { .. }
                    | OperationKind::ErrorField { .. }
                    | OperationKind::ErrorMatch { .. }
                    | OperationKind::ErrorIndex { .. }
                    | OperationKind::ErrorRest { .. }
                    | OperationKind::ThrowError { .. } => {
                        bail!("fixed Bytes execution requires the R3 data backend")
                    }
                    OperationKind::EnterScope { .. } | OperationKind::ExitScope { .. } => {
                        Value::Unit
                    }
                    OperationKind::PrepareNativeCall { .. }
                    | OperationKind::CloseNativeCall { .. }
                    | OperationKind::SettleNativeAdmissions { .. }
                    | OperationKind::ReadNativeResult { .. }
                    | OperationKind::ReleaseNativeCall { .. } => {
                        bail!("native callback ledger execution requires R4 backend")
                    }
                    OperationKind::EnterErrorGroup { scope } => {
                        frame.failure.enter_error_group(*scope);
                        Value::Unit
                    }
                    OperationKind::FinishErrorGroup { scope, handles } => {
                        let errors = frame.failure.finish_error_group(*scope, *handles)?;
                        for failure in &errors {
                            record_sync_caught(failure, worker, main, observed);
                        }
                        frame
                            .data_roots
                            .as_ref()
                            .ok_or_else(|| anyhow!("Error group requires Owner"))?
                            .owner
                            .borrow_mut()
                            .error_list(&errors)?
                    }
                    OperationKind::RecordTaskError { scope } => {
                        frame.failure.record_error(
                            *scope,
                            frame.error_origin.take(),
                            frame.block,
                        )?;
                        Value::Unit
                    }
                    OperationKind::ErrorGroupOutcome { cleanup, .. } => {
                        if let Some(failure) = frame.failure.finish_task_scope(*cleanup)? {
                            return Err(ManagedFailure(failure).into());
                        }
                        Value::Unit
                    }
                    OperationKind::CatchErrors { .. } => {
                        let failure = frame.failure.catch(block.id)?;
                        record_sync_caught(&failure, worker, main, observed);
                        frame
                            .data_roots
                            .as_ref()
                            .ok_or_else(|| anyhow!("List catch requires managed Owner values"))?
                            .owner
                            .borrow_mut()
                            .error_list(&[failure])?
                    }
                    OperationKind::CatchError { .. } => {
                        let failure = frame.failure.catch(block.id)?;
                        record_sync_caught(&failure, worker, main, observed);
                        Value::Error(failure)
                    }
                    OperationKind::ReserveSupplant { .. }
                    | OperationKind::StartSupplant { .. }
                    | OperationKind::SettleSupplant { .. } => {
                        bail!("supplant execution requires R4 reservation backend")
                    }
                    OperationKind::FunctionReference { target } => {
                        Value::FunctionReference(target.clone())
                    }
                    OperationKind::IndirectCall { .. } => {
                        unreachable!("resolved typed indirect call")
                    }
                    OperationKind::SourceNativeCall {
                        call: None,
                        signature,
                        arguments,
                        ..
                    } if main.is_some() => {
                        let arguments = arguments
                            .iter()
                            .zip(&signature.parameters)
                            .map(|(id, ty)| {
                                frame
                                    .values
                                    .get(id)
                                    .ok_or_else(|| anyhow!("service argument is unavailable"))
                                    .and_then(|value| {
                                        if matches!(ty, rinzu_compiler::SourceAbiType::Opaque(_)) {
                                            Ok(value.clone())
                                        } else {
                                            frame.export(value)
                                        }
                                    })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        main.unwrap().task.source_service(
                            signature,
                            arguments,
                            frame
                                .failure
                                .diagnostic(FailureCause::Error(ErrorKind::Io), operation.range)
                                .source,
                        )?
                    }
                    OperationKind::SourceNativeCall { .. } => {
                        bail!("source native execution requires R4 physical ABI backend")
                    }
                    OperationKind::NumericConvert { operand, .. } => {
                        let value = frame
                            .values
                            .get(operand)
                            .and_then(Value::scalar)
                            .ok_or_else(|| anyhow!("numeric conversion operand is not scalar"))?;
                        let target = crate::scalar_backend::kind(
                            operation.result.as_ref().unwrap().ty.scalar().unwrap(),
                        );
                        Value::Scalar(value.convert(target).map_err(|_| {
                            ManagedFailure(Failure::new(frame.failure.diagnostic(
                                FailureCause::Error(ErrorKind::Conversion),
                                operation.range,
                            )))
                        })?)
                    }
                    OperationKind::Parameter { index } => frame
                        .arguments
                        .get(usize::try_from(*index)?)
                        .cloned()
                        .ok_or_else(|| anyhow!("missing argument {index}"))?,
                    OperationKind::AsyncStart
                    | OperationKind::EnterTry { .. }
                    | OperationKind::LeaveTry { .. }
                    | OperationKind::GcSafepoint { .. } => return Ok(Value::Unit),
                    OperationKind::TakeTaskErrors { .. }
                    | OperationKind::EnterTaskScope { .. }
                    | OperationKind::JoinTaskScope { .. }
                    | OperationKind::TaskScopeOutcome { .. } => {
                        bail!("task scopes require Main async continuation execution");
                    }
                    OperationKind::TaskControl { kind, future } => {
                        let main =
                            main.ok_or_else(|| anyhow!("task control outside Main async"))?;
                        main.task
                            .task_control(*kind, future.and_then(|id| frame.values.get(&id)))?
                    }
                    OperationKind::CancellationPoll { .. } => {
                        if main.is_some_and(|m| {
                            m.task
                                .shared
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .shutdown_requested
                        }) {
                            cancellation.shutdown_requested.set(true);
                            cancellation.requested.set(true);
                        }
                        let cancelled = cancellation.poll(
                            frame.failure.in_cleanup(),
                            worker.is_some_and(|worker| {
                                worker.domain.is_some_and(WorkerDomain::is_cancel_requested)
                            }),
                        );
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        if cancellation.requested.get()
                            && let Some(roots) = &frame.data_roots
                        {
                            roots.owner.borrow().note_shutdown_request();
                        }
                        if cancelled {
                            return Err(ManagedFailure(Failure::new(
                                frame
                                    .failure
                                    .diagnostic(cancellation.cause(), operation.range),
                            ))
                            .into());
                        }
                        Value::Unit
                    }
                    OperationKind::Constant(value) => {
                        constant(value, &operation.result.as_ref().unwrap().ty)?
                    }
                    OperationKind::DeclareLocal { name } => {
                        frame.locals.remove(name);
                        Value::Unit
                    }
                    OperationKind::CheckLocal { name, .. } => {
                        if !frame.locals.contains_key(name) {
                            return Err(ManagedFailure(Failure::new(frame.failure.diagnostic(
                                FailureCause::Error(ErrorKind::UnboundValue),
                                operation.range,
                            )))
                            .into());
                        }
                        Value::Unit
                    }
                    OperationKind::LoadLocal { name } => frame
                        .locals
                        .get(name)
                        .cloned()
                        .ok_or_else(|| anyhow!("read of uninitialized local {name}"))?,
                    OperationKind::StoreLocal { name, value } => {
                        let value = frame
                            .values
                            .get(value)
                            .cloned()
                            .ok_or_else(|| anyhow!("store source value is unavailable"))?;
                        frame.locals.insert(name.clone(), value);
                        Value::Unit
                    }
                    OperationKind::Unary {
                        operator, operand, ..
                    } => unary(
                        *operator,
                        frame
                            .values
                            .get(operand)
                            .ok_or_else(|| anyhow!("unary source value is unavailable"))?,
                    )
                    .map_err(|_| {
                        ManagedFailure(Failure::new(
                            frame
                                .failure
                                .diagnostic(FailureCause::ArithmeticTrap, operation.range),
                        ))
                    })?,
                    OperationKind::Binary {
                        operator,
                        left,
                        right,
                        ..
                    } => binary(
                        *operator,
                        frame
                            .values
                            .get(left)
                            .ok_or_else(|| anyhow!("binary left value is unavailable"))?,
                        frame
                            .values
                            .get(right)
                            .ok_or_else(|| anyhow!("binary right value is unavailable"))?,
                    )
                    .map_err(|_| {
                        ManagedFailure(Failure::new(
                            frame
                                .failure
                                .diagnostic(FailureCause::ArithmeticTrap, operation.range),
                        ))
                    })?,
                    OperationKind::ManagedCall {
                        target, arguments, ..
                    } => {
                        let arguments = arguments
                            .iter()
                            .map(|value| {
                                frame
                                    .values
                                    .get(value)
                                    .cloned()
                                    .ok_or_else(|| anyhow!("call argument is unavailable"))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let mut inherited = cancellation.clone();
                        inherited.masked |= frame.failure.in_cleanup();
                        if let Some(main) = main {
                            evaluate_main_call(
                                module, target, arguments, &inherited, observed, main,
                            )?
                        } else {
                            evaluate_sync_context(
                                module, target, arguments, worker, &inherited, observed, None,
                            )?
                        }
                    }
                    OperationKind::StaticHandlerReference { target } => {
                        Value::Handler(target.clone())
                    }
                    OperationKind::Spawn {
                        target, arguments, ..
                    } if main.is_some() => {
                        let main = main.unwrap().task;
                        let arguments = arguments
                            .iter()
                            .map(|value| {
                                frame
                                    .values
                                    .get(value)
                                    .cloned()
                                    .ok_or_else(|| anyhow!("spawn argument is unavailable"))
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let prepared = PreparedSpawn::new(
                            module,
                            target,
                            &arguments,
                            &main.admission,
                            &frame.data_roots.as_ref().expect("Main Owner").owner,
                        );
                        let future = main
                            .shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .spawn(
                                main.child_scope,
                                target.clone(),
                                prepared,
                                frame
                                    .failure
                                    .diagnostic(FailureCause::Overloaded, operation.range)
                                    .source,
                                main.callback_admission,
                            )?;
                        main.start_child(future)?
                    }
                    OperationKind::NativeCall {
                        target, arguments, ..
                    } if main.is_some() => {
                        let main = main.unwrap().task;
                        let binding = main.native_async.get(target).ok_or_else(|| anyhow!("synchronous native call {target} requires the physical call backend"))?;
                        let request = match binding {
                            NativeAsyncOperation::FileReadSize { path_argument } => {
                                let value = arguments
                                    .get(*path_argument)
                                    .and_then(|value| frame.values.get(value));
                                let value = value.map(|value| frame.export(value)).transpose()?;
                                let Some(Value::String(path)) = value else {
                                    bail!("{target} path argument is not a String");
                                };
                                PendingRequest::FileReadSize {
                                    path: path.clone(),
                                    source: frame
                                        .failure
                                        .diagnostic(
                                            FailureCause::Error(ErrorKind::Io),
                                            operation.range,
                                        )
                                        .source,
                                }
                            }
                        };
                        let future = main
                            .shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .future_with_lease(
                                main.child_scope,
                                request,
                                main.callback_admission,
                            )?;
                        main.start_child(future)?
                    }
                    OperationKind::Await {
                        future, consume, ..
                    } if main.is_some() => {
                        let Some(Value::Future(future)) = frame.values.get(future) else {
                            bail!("await operand is not a Future")
                        };
                        let mut shared = main
                            .unwrap()
                            .task
                            .shared
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        let result = shared
                            .completed
                            .get(future)
                            .cloned()
                            .ok_or_else(|| anyhow!("pending callee requires a continuation"))?;
                        shared.observe(main.unwrap().task.scope, *future)?;
                        if *consume {
                            if let (Some(roots), Ok(value)) = (&frame.data_roots, &result) {
                                roots.hold(value);
                            }
                            shared.consume(*future)?;
                            if let Some(roots) = &frame.data_roots {
                                roots.owner.borrow_mut().release_result(*future);
                            }
                        }
                        let value = result.map_err(ManagedFailure)?;
                        shared.observe_awaited(main.unwrap().task.scope, *future, value.clone());
                        value
                    }
                    OperationKind::Await { .. }
                    | OperationKind::Spawn { .. }
                    | OperationKind::NativeCall { .. }
                    | OperationKind::StoreField { .. }
                    | OperationKind::Field { .. }
                    | OperationKind::ListElement { .. }
                    | OperationKind::EnumPayload { .. }
                    | OperationKind::Construct { .. } => {
                        bail!("worker function {name} contains an operation that may suspend")
                    }
                };
                Ok(value)
            })();
            let value = match value {
                Ok(value) => value,
                Err(error) => {
                    let Some(failure) = error.downcast_ref::<ManagedFailure>() else {
                        return Err(error);
                    };
                    frame.block = frame.propagate(operation, failure.0.clone())?;
                    frame.operation = 0;
                    continue 'blocks;
                }
            };
            if let Some(result) = &operation.result {
                frame.store_result(result.id, value, &result.ty)?;
            }
        }
        match &block.terminator {
            Terminator::Goto { target, .. } => {
                let target = frame.goto(block, *target)?;
                frame.jump(target);
            }
            Terminator::Branch {
                condition,
                then_target,
                else_target,
            } => {
                let condition = frame
                    .values
                    .get(condition)
                    .ok_or_else(|| anyhow!("branch condition is unavailable"))?;
                frame.block = if truthy(condition)? {
                    *then_target
                } else {
                    *else_target
                };
                frame.operation = 0;
            }
            Terminator::Return { value, .. } => {
                if let Some(target) = frame.defer_return(block, *value)? {
                    frame.jump(target);
                    continue 'blocks;
                }
                if let Some(failure) = frame.finish_failure()? {
                    return Err(ManagedFailure(failure).into());
                }
                if let Some((_, value)) = frame.deferred_return.take() {
                    return Ok(value);
                }
                return value
                    .map(|value| {
                        frame
                            .values
                            .get(&value)
                            .cloned()
                            .ok_or_else(|| anyhow!("return value is unavailable"))
                    })
                    .transpose()
                    .map(|value| value.unwrap_or(Value::Unit));
            }
            Terminator::Fail => {
                let failure = frame
                    .finish_failure()?
                    .ok_or_else(|| anyhow!("source failure block has no failure"))?;
                return Err(ManagedFailure(failure).into());
            }
            Terminator::Switch { value, cases, .. } if frame.data_roots.is_some() => {
                let variant = frame
                    .data_roots
                    .as_ref()
                    .expect("Main Owner")
                    .owner
                    .borrow()
                    .enum_variant(
                        frame
                            .values
                            .get(value)
                            .ok_or_else(|| anyhow!("missing enum value"))?,
                    )?;
                frame.block = cases
                    .iter()
                    .find(|(tag, _)| *tag == variant)
                    .ok_or_else(|| anyhow!("enum has no matching case"))?
                    .1;
                frame.operation = 0;
            }
            Terminator::Pending | Terminator::Unreachable | Terminator::Switch { .. } => {
                bail!("worker function {name} has an unsupported terminator")
            }
        }
    }
}

fn constant(value: &ConstantValue, ty: &rinzu_compiler::Type) -> Result<Value> {
    Ok(match value {
        ConstantValue::Unit => Value::Unit,
        ConstantValue::Bool(value) => Value::Scalar(ScalarValue::from_bool(*value)),
        ConstantValue::Int(value) => {
            let kind = crate::scalar_backend::kind(
                ty.scalar()
                    .ok_or_else(|| anyhow!("constant has no scalar type"))?,
            );
            Value::Scalar(
                ScalarValue::from_bits(kind, *value as u64)
                    .map_err(|error| anyhow!("invalid scalar literal: {error:?}"))?,
            )
        }
        ConstantValue::Float(value) => Value::Scalar(match ty.scalar() {
            Some(rinzu_compiler::AbiScalar::F32) => ScalarValue::from_f32(*value as f32),
            Some(rinzu_compiler::AbiScalar::F64) => ScalarValue::from_f64(*value),
            _ => bail!("floating literal has no precision"),
        }),
        ConstantValue::String(value) => Value::String(value.clone()),
        ConstantValue::Bytes(value) => Value::Bytes(value.clone()),
    })
}

fn unary(operator: UnaryOperator, operand: &Value) -> Result<Value> {
    match (operator, operand) {
        (UnaryOperator::Not, operand) => {
            Ok(Value::Scalar(ScalarValue::from_bool(!truthy(operand)?)))
        }
        (UnaryOperator::Negate, Value::Scalar(value)) => value
            .arithmetic(Arithmetic::Negate, *value)
            .map(Value::Scalar)
            .map_err(|error| anyhow!("scalar arithmetic trap: {error:?}")),
        _ => bail!("unsupported unary operation"),
    }
}

fn binary(operator: BinaryOperator, left: &Value, right: &Value) -> Result<Value> {
    if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
        let left = truthy(left)?;
        let right = truthy(right)?;
        return Ok(Value::Scalar(ScalarValue::from_bool(
            if operator == BinaryOperator::And {
                left && right
            } else {
                left || right
            },
        )));
    }
    if let (Value::Scalar(left), Value::Scalar(right)) = (left, right) {
        let comparison = match operator {
            BinaryOperator::Equal => Some(Comparison::Equal),
            BinaryOperator::NotEqual => Some(Comparison::NotEqual),
            BinaryOperator::Less => Some(Comparison::Less),
            BinaryOperator::LessEqual => Some(Comparison::LessEqual),
            BinaryOperator::Greater => Some(Comparison::Greater),
            BinaryOperator::GreaterEqual => Some(Comparison::GreaterEqual),
            _ => None,
        };
        return if let Some(comparison) = comparison {
            left.compare(comparison, *right)
                .map(|value| Value::Scalar(ScalarValue::from_bool(value)))
                .map_err(|error| anyhow!("invalid scalar comparison: {error:?}"))
        } else {
            let arithmetic = match operator {
                BinaryOperator::Add => Arithmetic::Add,
                BinaryOperator::Subtract => Arithmetic::Subtract,
                BinaryOperator::Multiply => Arithmetic::Multiply,
                BinaryOperator::Divide => Arithmetic::Divide,
                BinaryOperator::Remainder => Arithmetic::Remainder,
                _ => bail!("unsupported scalar arithmetic"),
            };
            left.arithmetic(arithmetic, *right)
                .map(Value::Scalar)
                .map_err(|error| anyhow!("scalar arithmetic trap: {error:?}"))
        };
    }
    match operator {
        BinaryOperator::Equal => Ok(Value::Scalar(ScalarValue::from_bool(left == right))),
        BinaryOperator::NotEqual => Ok(Value::Scalar(ScalarValue::from_bool(left != right))),
        _ => bail!("unsupported binary operation"),
    }
}

fn truthy(value: &Value) -> Result<bool> {
    match value {
        Value::Scalar(value) if value.kind() == ScalarKind::Bool => Ok(value.bits() != 0),
        _ => bail!("branch condition is not Bool"),
    }
}

fn utf8_length(value: &Value) -> Result<Value> {
    let Value::String(value) = value else {
        bail!("UTF-8 length operand is not a String");
    };
    Ok(Value::Scalar(
        ScalarValue::from_integer(ScalarKind::U64, value.len() as i128)
            .map_err(|error| anyhow!("invalid UTF-8 length: {error:?}"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_library::{
        compile_library as compile_source,
        compile_library_with_options as compile_source_with_options,
    };
    use rinzu_compiler::{
        CompilerOptions, FileId, NativeCallClass, NativeFunction, SourceFile, Type,
    };

    #[test]
    fn completion_published_between_empty_drain_and_wait_is_not_lost() {
        let signal = Arc::new(CompletionSignal::default());
        let (requested, requests) = std::sync::mpsc::channel();
        let (published, receipts) = std::sync::mpsc::channel();
        let publisher = Arc::clone(&signal);
        let helper = std::thread::spawn(move || {
            for _ in requests {
                publisher.publish();
                published.send(()).unwrap();
            }
        });
        let started = std::time::Instant::now();
        for _ in 0..256 {
            let observed = signal.epoch();
            requested.send(()).unwrap();
            receipts.recv().unwrap();
            signal.wait(observed, Duration::from_secs(5));
        }
        drop(requested);
        helper.join().unwrap();
        assert_eq!(signal.epoch(), 256);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn scalar_transport_preserves_all_kinds_and_exact_widths_in_source_workers() {
        for (name, literal, kind, bits) in [
            ("CInt8", "-128", ScalarKind::I8, (-128_i64) as u64),
            ("CInt16", "-32768", ScalarKind::I16, (-32768_i64) as u64),
            (
                "CInt32",
                "-2147483648",
                ScalarKind::I32,
                (-2147483648_i64) as u64,
            ),
            (
                "CInt64",
                "-9223372036854775808",
                ScalarKind::I64,
                i64::MIN as u64,
            ),
            ("CUInt8", "255", ScalarKind::U8, 255),
            ("CUInt16", "65535", ScalarKind::U16, 65535),
            ("CUInt32", "4294967295", ScalarKind::U32, 4294967295),
            ("CSize", "18446744073709551615", ScalarKind::U64, u64::MAX),
            (
                "CFloat",
                "-0.0",
                ScalarKind::F32,
                u64::from((-0.0_f32).to_bits()),
            ),
            ("CDouble", "-0.0", ScalarKind::F64, (-0.0_f64).to_bits()),
            ("CBool", "true", ScalarKind::Bool, 1),
        ] {
            let source = SourceFile::new(
                FileId(1),
                "typed-worker.rinzu",
                format!(
                    "fn echo(value: {name}) -> {name} {{ return value; }} async fn main() {{ let value: {name} = {literal}; let copy = await spawn echo(value); if copy != value {{ panic(\"failure\"); }} }}"
                ),
            );
            let artifact = compile_source(&source).unwrap();
            let run =
                run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
            let value = run.last_awaited_scalar.unwrap();
            assert_eq!(value.kind(), kind);
            assert_eq!(value.bits(), bits);
            assert_eq!(
                run.transport.peak_transport_bytes,
                kind.width() as usize / 8
            );
            assert!(run.resources_returned, "{run:?}");
        }
    }

    #[test]
    fn mixed_scalar_worker_arguments_use_ieee_arithmetic_and_separate_input_credit() {
        let source = SourceFile::new(
            FileId(1),
            "mixed-worker.rinzu",
            r#"
fn calculate(narrow: CInt8, wide: CUInt64, value: CFloat, enabled: CBool) -> CFloat {
    if narrow != -128 { panic("narrow_failure"); }
    if wide <= 9223372036854775808 { panic("unsigned_failure"); }
    if !enabled { panic("bool_failure"); }
    return (value + 1.0) - value;
}
async fn main() {
    let result = await spawn calculate(-128, 18446744073709551615, 16777216.0, true);
    if result != 0.0 { panic("rounding_failure"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert_eq!(run.last_awaited_scalar.unwrap().kind(), ScalarKind::F32);
        assert_eq!(run.last_awaited_scalar.unwrap().bits(), 0);
        assert_eq!(run.transport.peak_transport_bytes, 14);
        assert!(run.resources_returned);
    }

    #[test]
    fn ready_continuation_and_worker_preserve_nan_without_integer_reinterpretation() {
        let source = SourceFile::new(
            FileId(1),
            "nan-worker.rinzu",
            r#"
async fn ready() -> CFloat { return 0.0 / 0.0; }
fn echo(value: CFloat) -> CFloat {
    if value == value { panic("nan_failure"); }
    return value;
}
async fn main() {
    let value = await ready();
    let copy = await spawn echo(value);
    if copy == copy { panic("nan_failure"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        let value = run.last_awaited_scalar.unwrap();
        assert_eq!(value.kind(), ScalarKind::F32);
        assert!(value.float().unwrap().is_nan());
        let zero = ScalarValue::from_f32(0.0);
        assert_eq!(
            value.bits(),
            zero.arithmetic(Arithmetic::Divide, zero).unwrap().bits()
        );
        assert!(run.resources_returned);
    }

    #[test]
    fn worker_strings_and_utf8_length_copy_in_both_directions() {
        let source = SourceFile::new(
            FileId(1),
            "string-worker.rinzu",
            r#"
import rinzu.core;
fn echo(value: String) -> String { return value; }
fn length(value: String) -> CSize { return utf8ByteLength(value); }
async fn main() {
    let copy = await spawn echo("日本語🙂\u{0}tail");
    if copy != "日本語🙂\u{0}tail" { panic("copy_failure"); }
    let count = await spawn length(copy);
    if count != 18 { panic("length_failure"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert_eq!(run.last_awaited_scalar.unwrap().kind(), ScalarKind::U64);
        assert_eq!(run.last_awaited_result, Some(18));
        assert_eq!(run.transport.peak_transport_bytes, 26);
        assert!(run.resources_returned);
    }

    #[test]
    fn worker_traps_and_cancellation_return_transport_and_admission_to_baseline() {
        for (body, cancel) in [
            (
                "let byte: CUInt8 = 255; let overflow = byte + 1; return count;",
                false,
            ),
            (
                "var value = count; while true { value = value + 1; } return value;",
                true,
            ),
        ] {
            let source = SourceFile::new(
                FileId(1),
                "worker-failure.rinzu",
                format!(
                    "fn calculate(count: Int) -> Int {{ {body} }} async fn main() {{ let value = await spawn calculate(0); }}"
                ),
            );
            let artifact = compile_source(&source).unwrap();
            let admission = SpawnAdmissionController::new();
            assert!(
                run_source_async_admitted(
                    &artifact.ir,
                    "main",
                    SourceAsyncOptions {
                        cancel_first_spawn: cancel,
                        ..SourceAsyncOptions::default()
                    },
                    admission.clone()
                )
                .is_err()
            );
            let snapshot = admission.snapshot(OriginDomainId(0));
            assert_eq!(snapshot.worker_domains, 0);
            assert_eq!(snapshot.in_flight_for_origin, 0);
            assert_eq!(snapshot.transport_bytes, 0);
            if cancel {
                assert!([8, 26].contains(&snapshot.peak_transport_bytes));
            } else {
                assert_eq!(snapshot.peak_transport_bytes, 34);
            }
        }
    }

    #[test]
    fn explicit_ready_query_is_recorded_for_cancellation_comparison() {
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "ready-observation.rinzu",
            "async fn ready()->Int{return 42;} async fn main(){let pending=ready();let done=futureReady(pending);let value=await pending;if done||value!=42 {panic(\"wrong\");}}",
        ))
        .unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert_eq!(run.control_attempts.len(), 1);
        assert!(run.events.iter().any(|event| matches!(event,
            SourceAsyncEvent::Awaited {future, ..} if *future==run.control_attempts[0])));
    }

    #[test]
    fn first_await_starts_a_runtime_turn_and_ready_aliases_do_not_suspend_again() {
        let source = SourceFile::new(
            FileId(1),
            "ready-alias.rinzu",
            r#"
async fn ready()->Int {return 42;}
async fn invalid()->Int {let bad=CInt8(999);return 0;}
async fn main() {
    let value=ready();let alias=value;
    let first=await value;let second=await alias;
    if first!=42||second!=42 {panic("wrong");}
    let failure=invalid();var caught=0;
    try {let ignored=await failure;}catch one {caught=caught+1;}
    try {let ignoredAgain=await failure;}catch two {caught=caught+1;}
    if caught!=2 {panic("wrongCount");}
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run = run_source_async_report(&artifact.ir, "main", Default::default()).unwrap();
        assert!(run.failure_details.is_none() && run.resources_returned);
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(
            run.events
                .iter()
                .filter(|event| matches!(event, SourceAsyncEvent::Suspended { .. }))
                .count(),
            2,
        );
        let observed = run
            .events
            .iter()
            .filter_map(|event| match event {
                SourceAsyncEvent::Awaited { scope, future } => Some((*scope, *future)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(observed, [(0, 0), (0, 0), (0, 1), (0, 1)]);
    }

    #[test]
    fn source_spawn_suspends_and_resumes_on_owner_thread() {
        let source = SourceFile::from_utf8(
            FileId(1),
            "main.rinzu",
            br#"
fn cpuScore(size: Int) -> Int {
    return size * 2;
}

async fn main() {
    let score = await spawn cpuScore(20);
}
"#
            .to_vec(),
        )
        .unwrap();
        let artifact = compile_source(&source).unwrap();
        let owner = std::thread::current().id();
        let run = run_source_async(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert_eq!(run.result, 0);
        assert_eq!(run.last_awaited_result, Some(40));
        assert!(run.resources_returned);
        assert!(run.events.iter().any(|event| matches!(
            event,
            SourceAsyncEvent::WorkerCompleted { worker_thread, .. } if *worker_thread != owner
        )));
        assert!(run.events.iter().any(|event| matches!(
            event,
            SourceAsyncEvent::Resumed { owner_thread, .. } if *owner_thread == owner
        )));
    }

    #[test]
    fn source_file_read_uses_reactor_and_resumes_on_owner_thread() {
        let source = SourceFile::from_utf8(
            FileId(1),
            "main.rinzu",
            br#"
async fn main() {
    let size = await sdk.io.readSize("data.txt");
}
"#
            .to_vec(),
        )
        .unwrap();
        let mut compiler = CompilerOptions::default();
        compiler.native_functions.insert(
            "sdk.io.readSize".into(),
            NativeFunction {
                parameters: vec![Type::String],
                result: Type::Future(Box::new(Type::Int)),
                call_class: NativeCallClass::NativeSafe,
                metadata_complete: true,
                cleanup_safe: false,
            },
        );
        let artifact = compile_source_with_options(&source, &compiler).unwrap();
        let root = std::env::temp_dir().join(format!(
            "rinzu-source-async-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("data.txt"), b"rinzu").unwrap();
        let owner = std::thread::current().id();
        let run = run_source_async(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                package_root: root.clone(),
                native_async: BTreeMap::from([(
                    "sdk.io.readSize".into(),
                    NativeAsyncOperation::FileReadSize { path_argument: 0 },
                )]),
                ..SourceAsyncOptions::default()
            },
        )
        .unwrap();
        assert_eq!(run.last_awaited_result, Some(5));
        assert!(run.resources_returned);
        assert!(
            run.events
                .iter()
                .any(|event| matches!(event, SourceAsyncEvent::FileCompleted { bytes: 5, .. }))
        );
        assert!(run.events.iter().any(|event| matches!(
            event,
            SourceAsyncEvent::Resumed { owner_thread, .. } if *owner_thread == owner
        )));
        let submitted = run.events.iter().position(|event| matches!(event, SourceAsyncEvent::Submitted { operation, .. } if operation == "file-read-size")).unwrap();
        let awaited = run
            .events
            .iter()
            .position(|event| matches!(event, SourceAsyncEvent::Suspended { .. }))
            .unwrap();
        assert!(submitted < awaited);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_spawn_cancellation_is_not_detached() {
        let source = SourceFile::from_utf8(
            FileId(1),
            "main.rinzu",
            br#"
fn keepWorking(size: Int) -> Int {
    var value = 0;
    while value < size {
        value = value + 1;
    }
    return value;
}

async fn main() {
    let score = await spawn keepWorking(1000000);
}
"#
            .to_vec(),
        )
        .unwrap();
        let artifact = compile_source(&source).unwrap();
        let error = run_source_async(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                cancel_first_spawn: true,
                ..SourceAsyncOptions::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod sync_failure_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};

    #[test]
    fn legacy_native_callback_keeps_its_callers_owner_for_list_catches() {
        let module = Arc::new(
            compile_source(&SourceFile::new(
                FileId(179),
                "callback-errors.rinzu",
                r#"
            error A{} error B{}
            fn handler()->Int {
                try {try {throw A(message:"a");} finally {throw B(message:"b");}}
                catch errors {return 40+listLength(errors);}
                return 0;
            }
            async fn main(){}
        "#,
            ))
            .unwrap()
            .ir,
        );
        let owner = OwnerValues::new(&module).unwrap();
        let mut callback = SourceCallbackContext {
            scope: 0,
            module: Arc::clone(&module),
            shared: Arc::new(Mutex::new(SharedState::default())),
            owner: Rc::clone(&owner),
            source_native: Rc::new(NativeEngine::new(Arc::new(NativeBindings::default())).unwrap()),
        };
        let handler = function_index(&module, "handler").unwrap() as u64;
        // SAFETY: the synchronous invocation uses this live stack context and
        // cannot retain it after return; its Owner and module remain alive.
        let result =
            unsafe { invoke_source_handler(std::ptr::from_mut(&mut callback).cast(), handler) };
        assert_eq!(result, 42);
        let observed = &callback.shared.lock().unwrap().caught_errors;
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].cleanup.len(), 1);
        assert_eq!(owner.borrow_mut().finish().unwrap().objects, 0);
    }

    #[test]
    fn typed_sync_failure_routes_match_jit_calls_catches_and_nested_cleanup() {
        for body in [
            "try { let bad = fail(); } catch [error: Error, ...errorRest] { if error.message != \"numeric conversion is out of range or loses required integer precision\" { panic(\"wrong_message\"); } }",
            "var order = 0; try { try { let bad = fail(); } finally { order = order * 10 + 1; } } catch error { order = order * 10 + 2; } finally { order = order * 10 + 3; } if order != 123 { panic(\"wrong_order\"); }",
            "try { let bad = CInt8(200); } finally { let other = CUInt8(-1); }",
            "try { try { let bad = CInt8(200); } finally { let trap = 1 / 0; } } catch error { let should_not_catch = CInt8(300); }",
            "try { let trap = 1 / 0; } finally { try { let bad = CInt8(200); } catch [error: Error, ...errorRest] { let observed = error.message; } }",
            "try { let bad = CInt8(200); } finally { if true { try { let other = CUInt8(-1); } catch [error: Error, ...errorRest] { let observed = error.message; } } }",
            "try { try { let bad = CInt8(200); } finally { try { let second = CInt8(300); } finally { let third = CInt8(400); } } } catch [error: Error, ...errorRest] { let observed = error.message; }",
        ] {
            let source = SourceFile::new(
                FileId(23),
                "sync-failure.rinzu",
                format!("fn fail() -> CInt8 {{ return CInt8(200); }} fn main() {{ {body} }}"),
            );
            let artifact = compile_source(&source).unwrap();
            let jit = crate::run_baseline_jit(&artifact.ir, "main").unwrap();
            let mut observed = Vec::new();
            let owner = OwnerValues::new(&artifact.ir).unwrap();
            let native = NativeEngine::new(Arc::new(NativeBindings::default())).unwrap();
            let result = evaluate_sync_observed(
                &artifact.ir,
                "main",
                Vec::new(),
                Some(OwnerEvaluation {
                    domain: None,
                    callback: false,
                    owner: &owner,
                    source_native: &native,
                }),
                &mut observed,
            );
            let failure = result.as_ref().err().map(|error| {
                error
                    .downcast_ref::<ManagedFailure>()
                    .unwrap_or_else(|| panic!("unexpected failure in {body}: {error:#}"))
                    .0
                    .clone()
            });
            assert_eq!(failure, jit.failure_details, "{body}");
            assert_eq!(observed, jit.caught_errors, "{body}");
            assert_eq!(result.is_ok(), jit.result.is_ok(), "{body}");
            assert_eq!(owner.borrow_mut().finish().unwrap().objects, 0);
        }
    }
}

#[cfg(test)]
mod worker_failure_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};

    #[test]
    fn successful_return_break_and_continue_run_finally_and_keep_return_roots() {
        let source = SourceFile::new(
            FileId(190),
            "return-finally.rinzu",
            r#"
fn returning() -> String {
    try { try { return "retained"; }
        finally { try { let badInner = CInt8(300); } catch [inner: Error, ...innerRest] { let copiedInner = inner.message; } }
    } finally { try { let badOuter = CInt8(400); } catch [outer: Error, ...outerRest] { let copiedOuter = outer.message; } }
}
async fn ready() -> String {
    try { return "retained"; }
    finally { try { let badReady = CInt8(300); } catch [readyError: Error, ...readyErrorRest] { let copiedReady = readyError.message; } }
}
fn unpublished() -> String {
    try { return "unpublished"; } finally { let badCleanup = CInt8(200); }
}
async fn main() {
    let ordinary = returning(); let completed = await ready(); let worker = await spawn returning();
    if ordinary != "retained" || completed != ordinary || worker != ordinary { panic("lostReturn"); }
    try { let ignored = unpublished(); } catch [cleanupError: Error, ...cleanupErrorRest] { let cleanupMessage = cleanupError.message; }
    var count = 0;
    while count < 3 { try { continue; } finally { count = count + 1; } }
    while true { try { break; } finally { count = count + 1; } }
    if count != 4 { panic("wrongLoopCleanup"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(
            run.failure_details.is_none() && run.resources_returned,
            "{run:?}"
        );
        assert_eq!(run.caught_errors.len(), 6);
        assert!(
            run.caught_errors
                .iter()
                .all(|failure| failure.primary.cause == FailureCause::Error(ErrorKind::Conversion))
        );
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert!(
            run.worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
    }

    #[test]
    fn same_domain_future_preserves_producer_cleanup_and_repeated_observation() {
        let source = SourceFile::new(
            FileId(91),
            "future-errors.rinzu",
            r#"
async fn bad() -> CInt8 {
    try { return CInt8(200); }
    finally { try { let second = CInt8(300); } finally { let third = CInt8(400); } }
}
async fn main() {
    let future = bad(); let alias = future; var catches = 0;
    try { let ignored_one = await future; } catch one { catches = catches + 1; }
    try { let ignored_two = await alias; } catch two { catches = catches + 1; }
    if catches != 2 { panic("wrong_count"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(run.failure_details.is_none());
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(run.caught_errors[0], run.caught_errors[1]);
        let failure = &run.caught_errors[0];
        assert_eq!(
            failure.primary.cause,
            FailureCause::Error(ErrorKind::Conversion)
        );
        assert_eq!(failure.cleanup.len(), 2);
        assert!(
            failure
                .cleanup
                .iter()
                .all(|diagnostic| diagnostic.cause == FailureCause::Error(ErrorKind::Conversion))
        );
        assert!(
            failure.primary.source.unwrap().start_byte
                < failure.cleanup[0].source.unwrap().start_byte
        );
        assert!(
            failure.cleanup[0].source.unwrap().start_byte
                < failure.cleanup[1].source.unwrap().start_byte
        );
        assert_eq!(run.transport.peak_transport_bytes, 0);
        assert_eq!(
            run.events
                .iter()
                .filter(|event| matches!(event, SourceAsyncEvent::WorkerCompleted { .. }))
                .count(),
            0
        );
        assert!(run.resources_returned);
    }

    #[test]
    fn worker_handles_all_normal_errors_and_returns_a_transferable_report() {
        let source = SourceFile::new(
            FileId(92),
            "worker-catches.rinzu",
            r#"
fn bad() -> Int {
    try {
        try { let first = CInt8(200); }
        finally { try { let local = CInt8(300); } catch [caught: Error, ...caughtRest] { let message = caught.message; } let cleanup = CInt8(400); }
    } catch errors { return listLength(errors); }
    return 0;
}
async fn main() {
    let future = spawn bad();
    let first = await future;
    let second = await future;
    if first != 2 || second != first { panic("wrongReport"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert_eq!(run.caught_errors.len(), 2);
        assert!(run.caught_errors[0].cleanup.is_empty());
        assert_eq!(run.caught_errors[1].cleanup.len(), 1);
        // Private caught diagnostics also use transfer credit; no Error value
        // or unhandled normal failure crosses the Worker boundary.
        assert_eq!(run.transport.peak_transport_bytes, 94);
        assert!(run.resources_returned);
    }

    #[test]
    fn rejected_worker_result_releases_partial_packets_and_preserves_origin_source() {
        let source = SourceFile::new(
            FileId(93),
            "worker-credit.rinzu",
            r#"
fn bad(value: Int) -> String { return "result exceeds transport credit"; }
async fn main() { let value = await spawn bad(200); }
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let admission =
            SpawnAdmissionController::from_target_profile(rinzu_runtime::spawn::SpawnLimits {
                worker_domains: 1,
                in_flight_per_origin: 1,
                process_transport_bytes: 18,
                operation_transport_bytes: 18,
            });
        let run = run_source_async_report_admitted(
            &artifact.ir,
            "main",
            SourceAsyncOptions::default(),
            admission,
        )
        .unwrap();
        let failure = run.failure_details.unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Overloaded);
        assert!(failure.primary.source.is_some());
        assert_eq!(run.transport.peak_transport_bytes, 8);
        assert!(run.resources_returned);
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};

    const WORKER: &str = r#"
fn work() -> Int {
    try { var value = 0; while true { value = value + 1; } return value; }
    finally {
        try { let local = CInt8(300); } catch [inside: Error, ...insideRest] { let message = inside.message; }
        try { let cleanup = CInt8(400); } finally { let other = CInt8(500); }
    }
}
"#;
    const MAIN: &str = r#"
async fn main() {
    try { let value = await spawn work(); }
    catch wrong { let should_not_catch = CInt8(900); }
    finally { let cleanup = CInt8(600); }
}
"#;

    #[test]
    fn worker_cancellation_preserves_primary_and_runs_worker_then_origin_cleanup() {
        let source = SourceFile::new(FileId(51), "cancel-worker.rinzu", format!("{WORKER}{MAIN}"));
        let artifact = compile_source(&source).unwrap();
        for limit in [2, 3, 7] {
            let worker = crate::run_baseline_jit_with_options(
                &artifact.ir,
                "work",
                crate::BaselineOptions {
                    cancel_after_polls: Some(limit),
                    ..crate::BaselineOptions::default()
                },
            )
            .unwrap();
            let run = run_source_async_report(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    cancel_worker_after_polls: Some(limit),
                    ..SourceAsyncOptions::default()
                },
            )
            .unwrap();
            let failure = run.failure_details.as_ref().unwrap();
            assert_eq!(
                failure.primary,
                worker.failure_details.as_ref().unwrap().primary
            );
            assert_eq!(failure.primary.cause, FailureCause::Cancelled);
            assert_eq!(failure.cleanup.len(), 3);
            assert_eq!(
                failure.cleanup[..2],
                worker.failure_details.unwrap().cleanup
            );
            assert_eq!(run.caught_errors, worker.caught_errors);
            assert_eq!(run.transport.peak_transport_bytes, 104);
            assert!(run.resources_returned);
        }
    }

    #[test]
    fn cancellation_winner_waits_for_worker_resources_and_origin_finally() {
        let source = SourceFile::new(FileId(52), "cancel-winner.rinzu", format!("{WORKER}{MAIN}"));
        let artifact = compile_source(&source).unwrap();
        for _ in 0..16 {
            let run = run_source_async_report(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    cancel_first_spawn: true,
                    ..SourceAsyncOptions::default()
                },
            )
            .unwrap();
            let failure = run.failure_details.unwrap();
            assert_eq!(failure.primary.cause, FailureCause::Cancelled);
            assert!(failure.primary.source.is_some());
            let cleanup = failure.cleanup.last().unwrap();
            assert_eq!(cleanup.cause, FailureCause::Error(ErrorKind::Conversion));
            assert!(
                cleanup.source.unwrap().start_byte > failure.primary.source.unwrap().start_byte
            );
            assert!(
                run.events
                    .iter()
                    .any(|event| matches!(event, SourceAsyncEvent::Cancelled { .. }))
            );
            assert!(run.resources_returned);
        }
    }
}

#[cfg(test)]
mod child_close_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};

    #[test]
    fn main_task_admission_rejects_before_prefix_and_physically_joins_admitted_children() {
        let file = FileId(381);
        let source = SourceFile::new(
            file,
            "main-limit.rinzu",
            r#"
import rinzu.stdio;
import rinzu.timer;
async fn pause(milliseconds: Int) -> Unit { await rinzu.timer.sleep(milliseconds); }

class Count {value:Int}
async fn child(count:Count) {count.value=count.value+1;await pause(2);}
async fn main() {let count=Count(value:0);try all {let first=child(count);let second=child(count);let third=child(count);let rejected=child(count);await rejected;await first;await second;await third;} finally {rinzu.stdio.writeLine(f"{count.value}");}}
"#,
        );
        let mut options = rinzu_compiler::CompilerOptions::default();
        options.sdk_files.insert(file);
        let artifact =
            crate::test_library::compile_library_with_options(&source, &options).unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                capture_stdout: true,
                main_task_limit: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.future_admissions.len(), 3);
        assert_eq!(run.stdout, b"0\n");
        assert_eq!(
            run.failure_details.as_ref().unwrap().primary.cause,
            FailureCause::Overloaded
        );
        assert!(run.resources_returned);
    }

    #[test]
    fn saturated_mailbox_keeps_native_drain_and_cancellation_until_physical_completion() {
        use rinzu_runtime::native_completion::{CompletionPointer, NativeCompletion};
        use std::sync::atomic::{AtomicU32, Ordering};
        let module = Arc::new(
            compile_source(&SourceFile::new(
                FileId(1),
                "drain.rinzu",
                "async fn main(){} ",
            ))
            .unwrap()
            .ir,
        );
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let owner = OwnerValues::new(&module).unwrap();
        let mut operations = SourceOperations::new(
            Arc::clone(&module),
            Arc::clone(&shared),
            &SourceAsyncOptions::default(),
            &owner,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Arc::new(NativeBindings::default()),
        );
        operations.mailbox = OperationMailbox::new(
            rinzu_runtime::resources::ResourceLimits {
                pending_operations: 0,
                ..Default::default()
            },
            Arc::new(|| {}),
        );
        let Value::Future(rejected) = shared
            .lock()
            .unwrap()
            .future(
                0,
                PendingRequest::Timer {
                    milliseconds: 1,
                    source: None,
                },
            )
            .unwrap()
        else {
            unreachable!()
        };
        let rejection = operations.start(rejected).unwrap_err();
        assert_eq!(
            rejection
                .downcast_ref::<ManagedFailure>()
                .unwrap()
                .0
                .primary
                .cause,
            FailureCause::Overloaded
        );
        {
            let state = shared.lock().unwrap();
            assert!(
                state.future_scopes.is_empty()
                    && state.future_admissions.is_empty()
                    && state.pending.is_empty()
                    && state.completed.is_empty()
            );
        }
        assert!(operations.mailbox.resources().used.is_baseline());
        let native = Box::new(NativeCompletion {
            version: 1,
            size: 16,
            done: AtomicU32::new(0),
            cancelled: AtomicU32::new(0),
        });
        // SAFETY: the Box stays pinned through completion and receipt below.
        let completion =
            unsafe { CompletionPointer::new((&*native as *const NativeCompletion) as usize) };
        let Value::Future(future) = shared
            .lock()
            .unwrap()
            .future(
                0,
                PendingRequest::NativeCompletion {
                    completion,
                    transfer: false,
                    source: None,
                },
            )
            .unwrap()
        else {
            unreachable!()
        };
        operations.start(future).unwrap();
        assert!(operations.deferred_native.contains(&future));
        assert!(!shared.lock().unwrap().completed.contains_key(&future));
        assert!(operations.request_control(future, false));
        assert_eq!(native.cancelled.load(Ordering::Acquire), 1);
        assert!(!operations.receive_one_ready().unwrap());
        native.done.store(1, Ordering::Release);
        assert!(operations.receive_one_ready().unwrap());
        assert!(operations.deferred_native.is_empty());
        assert_eq!(
            shared.lock().unwrap().completed[&future]
                .as_ref()
                .unwrap_err()
                .primary
                .cause,
            FailureCause::Cancelled
        );
        assert!(operations.mailbox.resources().used.is_baseline());
        for cancelled in [false, true] {
            use rinzu_runtime::native_completion::{
                NotifiedNativeCompletion, TransferNativeCompletion,
            };
            let mut carrier = Box::new(TransferNativeCompletion::default());
            carrier.result = i64::MIN + 65536;
            // SAFETY: the Box remains pinned through the deferred physical wait.
            let completion = unsafe {
                CompletionPointer::new((&*carrier as *const TransferNativeCompletion) as usize)
            };
            let Value::Future(future) = shared
                .lock()
                .unwrap()
                .future(
                    0,
                    PendingRequest::NativeCompletion {
                        completion,
                        transfer: true,
                        source: None,
                    },
                )
                .unwrap()
            else {
                unreachable!()
            };
            operations.start(future).unwrap();
            assert!(operations.deferred_native.contains(&future));
            if cancelled {
                assert!(operations.request_control(future, false));
            }
            assert!(!operations.receive_one_ready().unwrap());
            // SAFETY: valid uniquely published v3 prefix, no producer access follows.
            unsafe {
                NotifiedNativeCompletion::publish(&mut carrier.completion);
            }
            assert!(operations.receive_one_ready().unwrap());
            let completed = shared.lock().unwrap().completed[&future].clone();
            drop(carrier);
            if cancelled {
                assert_eq!(
                    completed.unwrap_err().primary.cause,
                    FailureCause::Cancelled
                );
            } else {
                assert_eq!(completed.unwrap(), Value::int(i64::MIN + 65536));
            }
            assert!(operations.mailbox.resources().used.is_baseline());
        }
        drop(operations);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn source_child_join_cycle_fails_before_return_or_owner_disposal() {
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","source_async::child_close_tests::source_prefix_starts_children_without_await_and_receives_each_actual_operation","--nocapture"])
            .env("RINZU_SOURCE_WAIT_CYCLE_PROBE","self").output().unwrap();
        assert_eq!(output.status.code(), Some(86), "{output:?}");
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(
            diagnostic.contains("observable-wait-cycle")
                && diagnostic.contains("symbol=main ")
                && diagnostic.contains("task=1 dependency=1")
                && !diagnostic.contains("source=0:"),
            "{diagnostic}"
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("test result: ok"));
    }

    #[test]
    fn source_prefix_starts_children_without_await_and_receives_each_actual_operation() {
        let source = SourceFile::new(
            FileId(70),
            "eager-prefix.rinzu",
            r#"
fn echo(value: Int) -> Int { return value; }
async fn main() { let first = spawn echo(7); let second = spawn echo(8); }
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let module = Arc::new(artifact.ir);
        let entry = function_index(&module, "main").unwrap();
        let shared = Arc::new(Mutex::new(SharedState::default()));
        let continuations = Rc::new(RefCell::new(BTreeMap::new()));
        let owner_values = OwnerValues::new(&module).unwrap();
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        let source_bindings = Arc::new(NativeBindings::default());
        let operations = Rc::new(RefCell::new(SourceOperations::new(
            Arc::clone(&module),
            Arc::clone(&shared),
            &SourceAsyncOptions::default(),
            &owner_values,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Arc::clone(&source_bindings),
        )));
        let admission = SpawnAdmissionController::new();
        let mut task = SourceTask {
            module: Arc::clone(&module),
            native_async: BTreeMap::new(),
            frame: Frame::new(&module, entry, &module.functions[entry], Vec::new())
                .with_owner(&owner_values),
            shared: Arc::clone(&shared),
            owner_thread: std::thread::current().id(),
            native_packages: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            source_native: Rc::new(NativeEngine::new(source_bindings).unwrap()),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            supplants: Weak::new(),
            cancellation: CancellationState::default(),
            admission: admission.clone(),
            continuations: Rc::downgrade(&continuations),
            operations: Rc::downgrade(&operations),
            // Private state-corruption probe: the first child's identity is
            // deliberately aliased to the parent, exercising real join registration.
            scope: if std::env::var_os("RINZU_SOURCE_WAIT_CYCLE_PROBE").is_some() {
                1
            } else {
                0
            },
            completion: None,
            closing: None,
            closing_child: None,
            closing_group: None,
            task_groups: Vec::new(),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            readiness: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            wait_dependency: None,
        };
        assert!(matches!(
            task.resume(TaskEvent::Start),
            TaskAction::Pending {
                cleanup_safe: true,
                ..
            }
        ));
        {
            let shared = shared.lock().unwrap();
            assert!(
                shared
                    .events
                    .iter()
                    .all(|event| matches!(event, SourceAsyncEvent::Submitted { .. }))
            );
            assert_eq!(shared.events.len(), 2);
            assert!(shared.completed.is_empty() && shared.result.is_none());
        }
        // Both Worker bodies complete while the Main has registered no source
        // await. Each native packet and its admission stay held for its Owner.
        let mut services = operations.borrow_mut();
        for _ in 0..2 {
            let completion = services
                .workers
                .wait_completion(Duration::from_secs(2))
                .unwrap();
            services
                .worker_completions
                .insert(completion.operation, completion);
        }
        let snapshot = admission.snapshot(OriginDomainId(0));
        assert_eq!(snapshot.worker_domains, 2);
        assert_eq!(snapshot.in_flight_for_origin, 2);
        assert_eq!(snapshot.transport_bytes, 16);
        assert!(!services.mailbox.resources().used.is_baseline());
        // Receive in reverse creation order using the real completion IDs.
        services.receive(1).unwrap();
        assert!(shared.lock().unwrap().result.is_none());
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .completed
                .get(&1)
                .unwrap()
                .as_ref()
                .unwrap()
                .as_i64()
                .unwrap(),
            8
        );
        services.receive(0).unwrap();
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .completed
                .get(&0)
                .unwrap()
                .as_ref()
                .unwrap()
                .as_i64()
                .unwrap(),
            7
        );
        assert!(services.is_baseline());
        drop(services);
        assert!(matches!(
            task.resume(TaskEvent::Operation(Resolution::Completed(Ok(())))),
            TaskAction::Return
        ));
        assert!(shared.lock().unwrap().result.as_ref().unwrap().is_ok());
        assert_eq!(admission.snapshot(OriginDomainId(0)).transport_bytes, 0);
    }

    #[test]
    fn sync_launchers_keep_child_scope_and_catch_chronology_across_pending_prefixes() {
        let text = r#"
fn echo() -> Int { return 7; }
async fn delayed() -> Int {
    try { let prefixFailure = CInt8(200); } catch prefix { }
    let value = await spawn echo();
    try { let resumedFailure = CInt8(300); } catch resumed { }
    return value;
}
fn launch() -> Int {
    try { let beforeFailure = CInt8(400); } catch before { }
    let child = delayed();
    try { let afterFailure = CInt8(500); } catch after { }
    let second = spawn echo();
    return 7;
}
async fn main() { let value = launch(); }
"#;
        let source = SourceFile::new(FileId(68), "sync-launcher.rinzu", text);
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert_eq!(
            run.caught_errors
                .iter()
                .map(|failure| failure.primary.source.unwrap().start_byte as usize)
                .collect::<Vec<_>>(),
            [400, 500, 200, 300].map(|value| text.find(&format!("CInt8({value})")).unwrap())
        );
        assert_eq!(
            run.events
                .iter()
                .filter(|event| matches!(event, SourceAsyncEvent::WorkerCompleted { .. }))
                .count(),
            2
        );
        assert!(run.resources_returned);
    }

    #[test]
    fn pending_file_callees_resume_with_values_and_immutable_io_failures() {
        let source = SourceFile::new(
            FileId(69),
            "pending-file.rinzu",
            r#"
async fn read(path: String) -> Int { return await sdk.io.readSize(path); }
async fn main() {
    try all {
    let future = read("data.txt");
    let missing = read("missing.txt"); let alias = missing;
    let size = await future;
    if size != 5 { panic("wrong_size"); }
    try { let first = await missing; } catch [one: Error, ...oneRest] { let firstMessage = one.message; }
    try { let second = await alias; } catch [two: Error, ...twoRest] { let secondMessage = two.message; }
    } catch errors {}
}
"#,
        );
        let mut compiler = rinzu_compiler::CompilerOptions::default();
        compiler.native_functions.insert(
            "sdk.io.readSize".into(),
            rinzu_compiler::NativeFunction {
                parameters: vec![rinzu_compiler::Type::String],
                result: rinzu_compiler::Type::Future(Box::new(rinzu_compiler::Type::Int)),
                call_class: rinzu_compiler::NativeCallClass::NativeSafe,
                metadata_complete: true,
                cleanup_safe: false,
            },
        );
        let artifact =
            crate::test_library::compile_library_with_options(&source, &compiler).unwrap();
        let root = std::env::temp_dir().join(format!(
            "rinzu-pending-file-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("data.txt"), b"rinzu").unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                package_root: root.clone(),
                native_async: BTreeMap::from([(
                    "sdk.io.readSize".into(),
                    NativeAsyncOperation::FileReadSize { path_argument: 0 },
                )]),
                ..SourceAsyncOptions::default()
            },
        )
        .unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(run.caught_errors[0], run.caught_errors[1]);
        assert_eq!(
            run.caught_errors[0].primary.cause,
            FailureCause::Error(ErrorKind::Io)
        );
        assert_eq!(run.awaited_scalars.len(), 2);
        assert!(run.awaited_scalars.iter().all(|value| value.bits() == 5));
        assert!(run.resources_returned);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_callees_keep_their_prefix_and_resume_each_dependency_before_parent_completion() {
        let source = SourceFile::new(
            FileId(65),
            "pending-callee.rinzu",
            r#"
fn echo(value: Int) -> Int { return value; }
async fn delayed(value: Int) -> Int {
    let first = await spawn echo(value);
    let second = await spawn echo(first + 1);
    return second;
}
async fn nested(value: Int) -> Int { return await delayed(value); }
async fn main() {
    var value = 7;
    let future = nested(value); value = 100;
    let copy = await future;
    if copy != 8 || value != 100 { panic("wrong_value"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert_eq!(
            run.awaited_scalars
                .iter()
                .map(|value| value.bits())
                .collect::<Vec<_>>(),
            [7, 8, 8, 8]
        );
        let flow = run
            .events
            .iter()
            .filter_map(|event| match event {
                SourceAsyncEvent::Suspended {
                    future, operation, ..
                } => Some(format!("pending:{future}:{operation}")),
                SourceAsyncEvent::WorkerCompleted { future, .. } => {
                    Some(format!("worker:{future}"))
                }
                SourceAsyncEvent::Resumed { future, .. } => Some(format!("ready:{future}")),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            flow,
            [
                "pending:0:async",
                "pending:1:async",
                "pending:2:spawn",
                "worker:2",
                "ready:2",
                "pending:3:spawn",
                "worker:3",
                "ready:3",
                "ready:1",
                "ready:0"
            ]
        );
        assert!(run.resources_returned);
    }

    #[test]
    fn abandoned_failures_are_rejected_and_unstarted_children_cancel_without_cleanup() {
        let functions = r#"
fn echo() -> Int { return 7; }
fn forever() -> Int { while true { } return 0; }
async fn failed() -> Int {
    try { let first = await spawn echo(); let failure = CInt8(200); return 7; }
    finally { let cleanup = CInt8(300); }
}
async fn waiting() -> Int {
    try { let value = await spawn forever(); return value; }
    finally { try {let cleanup = CInt8(400);}catch errors{} }
}
async fn joining() -> Int { let child = waiting(); return 7; }
"#;
        for (body, cause, cleanup) in [
            (
                "let child = failed();",
                FailureCause::Error(ErrorKind::UnobservedChildFailure),
                2,
            ),
            (
                "let child = waiting(); let failure = 1 / 0;",
                FailureCause::ArithmeticTrap,
                0,
            ),
            (
                "let child = joining(); let failure = 1 / 0;",
                FailureCause::ArithmeticTrap,
                0,
            ),
            (
                "try { let child = joining(); let failure = 1 / 0; } finally { let outerCleanup = CInt8(500); }",
                FailureCause::ArithmeticTrap,
                1,
            ),
        ] {
            let source = SourceFile::new(
                FileId(66),
                "pending-close.rinzu",
                format!("{functions}\nasync fn main() {{ {body} }}"),
            );
            if body == "let child = failed();" {
                let errors = compile_source(&source).unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|error| error.code == rinzu_compiler::DiagnosticCode::FutureDiscarded),
                    "{errors:#?}"
                );
                continue;
            }
            let artifact = compile_source(&source).unwrap();
            let run = run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default())
                .unwrap();
            let failure = run.failure_details.as_ref().unwrap();
            assert_eq!(failure.primary.cause, cause, "{body}: {run:?}");
            assert_eq!(failure.cleanup.len(), cleanup, "{body}: {run:?}");
            assert!(run.resources_returned, "{body}: {run:?}");
            if cause == FailureCause::ArithmeticTrap {
                assert!(
                    run.events
                        .iter()
                        .all(|event| !matches!(event, SourceAsyncEvent::Submitted { .. })),
                    "a child cancelled before its first turn must not enter a Worker",
                );
            }
        }
    }

    #[test]
    fn failed_pending_callee_is_observed_only_at_await_and_keeps_its_snapshot() {
        let source = SourceFile::new(
            FileId(67),
            "pending-observation.rinzu",
            r#"
fn echo() -> Int { return 7; }
async fn failed() -> CInt8 {
    try { let first = await spawn echo(); return CInt8(200); }
    finally { let cleanup = CInt8(300); }
}
async fn main() {
    var order = 0;
    try {
        let future = failed(); let alias = future; order = 1;
        try { let first = await future; } catch one { order = order * 10 + 2; }
        try { let second = await alias; } catch two { order = order * 10 + 3; }
    } catch wrong { order = 99; }
    if order != 123 { panic("wrong_order"); }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(run.caught_errors[0], run.caught_errors[1]);
        assert_eq!(
            run.caught_errors[0].primary.cause,
            FailureCause::Error(ErrorKind::Conversion)
        );
        assert_eq!(run.caught_errors[0].cleanup.len(), 1);
        assert!(run.resources_returned);
    }

    #[test]
    fn spawn_reserves_at_creation_and_rejection_does_not_return_a_future() {
        let source = SourceFile::new(
            FileId(64),
            "spawn-admission.rinzu",
            r#"
fn echo(value: Int) -> Int { return value; }
async fn main() {
    var prefix = 0;
    try {
        let first = spawn echo(7);
        let denied = spawn echo(8);
        prefix = 1;
        let value = await denied;
    } catch wrong { prefix = 99; }
    finally {
        if prefix != 0 { panic("wrong_prefix"); }
        let cleanup = CInt8(300);
    }
}
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let admission =
            SpawnAdmissionController::from_target_profile(rinzu_runtime::spawn::SpawnLimits {
                worker_domains: 1,
                ..rinzu_runtime::spawn::SpawnLimits::default()
            });
        let run = run_source_async_report_admitted(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                record_transport_evidence: true,
                ..SourceAsyncOptions::default()
            },
            admission,
        )
        .unwrap();
        assert_eq!(run.future_admissions.len(), 1);
        let failure = run.failure_details.unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Overloaded);
        let denied_source = artifact
            .ir
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.operations)
            .filter(|operation| matches!(operation.kind, OperationKind::Spawn { .. }))
            .nth(1)
            .unwrap()
            .range;
        assert_eq!(
            failure.primary.source.unwrap().start_byte,
            denied_source.start_byte
        );
        assert_eq!(failure.cleanup.len(), 1);
        assert_eq!(
            failure.cleanup[0].cause,
            FailureCause::Error(ErrorKind::Conversion)
        );
        assert!(run.caught_errors.is_empty());
        assert!(
            run.events
                .iter()
                .any(|event| matches!(event, SourceAsyncEvent::Submitted { future: 0, .. }))
        );
        assert!(
            run.events
                .iter()
                .all(|event| !matches!(event, SourceAsyncEvent::Suspended { .. }))
        );
        assert_eq!(run.transport.peak_worker_domains, 1);
        assert_eq!(run.transport.peak_in_flight_per_origin, 1);
        // A started child may return Int or observe cancellation and return a
        // diagnostic packet. Verify the actual credit intervals for either path.
        use rinzu_runtime::transport::TransportDirection;
        let input = run
            .transport_evidence
            .iter()
            .filter(|credit| credit.direction == TransportDirection::Input)
            .collect::<Vec<_>>();
        assert_eq!(input.len(), 1);
        assert_eq!((input[0].operation, input[0].bytes), (Some(0), 8));
        let result = run
            .transport_evidence
            .iter()
            .filter(|credit| credit.direction == TransportDirection::Result)
            .collect::<Vec<_>>();
        assert!(result.len() <= 1);
        let cancelled_bytes = rinzu_runtime::transport::preflight(
            &failure_descriptor(),
            &failure_value(&Failure::new(FailureCause::Cancelled)),
        )
        .unwrap();
        for credit in result {
            assert_eq!(credit.operation, Some(0));
            assert!(
                credit.bytes == 8
                    || (run.cancelled_children.contains(&0) && credit.bytes == cancelled_bytes)
            );
            assert!(input[0].released_at.unwrap() < credit.reserved_at);
        }
        let mut changes = run
            .transport_evidence
            .iter()
            .flat_map(|credit| {
                [
                    (credit.reserved_at, credit.bytes as i64),
                    (credit.released_at.unwrap(), -(credit.bytes as i64)),
                ]
            })
            .collect::<Vec<_>>();
        changes.sort_by_key(|(sequence, _)| *sequence);
        let mut used = 0i64;
        let mut peak = 0i64;
        for (_, change) in changes {
            used += change;
            assert!(used >= 0);
            peak = peak.max(used);
        }
        assert_eq!(used, 0);
        assert_eq!(run.transport.peak_transport_bytes, peak as usize);
        assert!(run.resources_returned);
    }

    #[test]
    fn a_returned_future_keeps_its_failure_independent_of_the_completed_parent() {
        let text = r#"
class Pending {value:Future<CInt8>}
async fn bad()->CInt8 {try{return CInt8(200);}finally{let cleanup=CInt8(300);}}
fn launch()->Pending {return Pending(value:bad());}
async fn parent()->Pending {return launch();}
async fn main() {
    let holder=await parent();let alias=holder.value;
    try{let first=await holder.value;}catch errors{}
    try{let second=await alias;}catch errors{}
}
"#;
        let artifact =
            compile_source(&SourceFile::new(FileId(63), "returned-future.rinzu", text)).unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            run.failure_details.is_none() && run.resources_returned,
            "{run:#?}"
        );
        assert_eq!(run.caught_errors.len(), 2);
        assert_eq!(run.caught_errors[0], run.caught_errors[1]);
        assert_eq!(run.caught_errors[0].cleanup.len(), 1);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    }

    #[test]
    fn unhandled_worker_normal_errors_are_rejected_before_runtime_admission() {
        let source = SourceFile::new(
            FileId(61),
            "unawaited-worker.rinzu",
            r#"
fn bad() -> CInt8 { try { return CInt8(200); } finally { let cleanup = CInt8(300); } }
async fn main() {
    let pending = spawn bad();
    try { let ignored = await pending; } catch errors {}
}
"#,
        );
        let errors = compile_source(&source).unwrap_err();
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.code == rinzu_compiler::DiagnosticCode::ErrorEscapesBoundary
                    && diagnostic.message.contains("Worker")
            }),
            "{errors:#?}"
        );
    }

    #[test]
    fn parent_failure_cancels_running_child_without_replacing_its_primary() {
        let source = SourceFile::new(
            FileId(62),
            "close-failure.rinzu",
            r#"
fn forever() -> Int { while true { } return 0; }
async fn main() { let child = spawn forever(); try { let failure = 1 / 0; } finally { let cleanup = CInt8(300); } }
"#,
        );
        let artifact = compile_source(&source).unwrap();
        let run =
            run_source_async_report(&artifact.ir, "main", SourceAsyncOptions::default()).unwrap();
        let failure = run.failure_details.unwrap();
        assert_eq!(failure.primary.cause, FailureCause::ArithmeticTrap);
        assert_eq!(failure.cleanup.len(), 1);
        assert_eq!(run.transport.peak_worker_domains, 1);
        assert!(run.resources_returned);
    }
}

#[cfg(test)]
mod owner_heap_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};

    #[test]
    fn typed_function_references_execute_on_main_workers_and_pending_callees() {
        let source = format!(
            "{}\nfn safeVerify()->Int {{try{{return verify();}}catch errors{{return 0;}}}} async fn forwarded() -> Int {{ return await spawn safeVerify(); }} async fn main() {{ let on_main=verify(); let reference=forwarded; let pending=reference(); let on_worker=await pending; if on_main!=42 || on_worker!=42 {{ panic(\"bad_result\"); }} }}",
            include_str!("../tests/fixtures/function_reference.rinzu")
                .replace("fn main()", "fn unused_main()")
        );
        let artifact =
            compile_source(&SourceFile::new(FileId(1), "reference-owner.rinzu", source)).unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            run.failure_details.is_none() && run.resources_returned,
            "{run:?}"
        );
        assert_eq!(run.caught_errors.len(), 4);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert_eq!(run.worker_heaps.len(), 1);
        assert_eq!(
            (
                run.worker_heaps[0].heap.objects,
                run.worker_heaps[0].heap.roots
            ),
            (0, 0)
        );
        assert!(
            run.worker_heaps[0].heap.collections > 50 && run.worker_heaps[0].heap.peak_frames >= 3
        );
    }

    #[test]
    fn main_and_sync_frames_execute_composites_bytes_and_gc_on_one_owner_heap() {
        for source in [
            include_str!("../tests/fixtures/composite.rinzu"),
            include_str!("../tests/fixtures/bytes_collection.rinzu"),
        ] {
            let source = source.replace("fn main()", "async fn main()");
            let artifact =
                compile_source(&SourceFile::new(FileId(1), "owner-heap.rinzu", source)).unwrap();
            let run = run_source_async(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    gc_stress: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(run.result, 0);
            assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
            assert!(run.resources_returned);
            assert!(run.heap.collections > 50, "{run:?}");
            assert!(run.heap.moved_objects > 100, "{run:?}");
            assert!(run.heap.peak_frames >= 3, "{run:?}");
            assert!(run.heap.peak_objects < 100, "{run:?}");
        }
    }
}

#[cfg(test)]
mod composite_worker_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};
    use rinzu_runtime::transport::TransportDirection;

    #[test]
    fn rejected_composite_results_and_partial_diagnostics_release_the_real_worker_heap() {
        let functions = r#"
class Packet { const text: String, const bytes: Bytes(4), const high: CSize }
fn expanded(value: Packet) -> List<Packet> { return [value,value,value,value]; }
fn observed(value: Packet) -> Packet {
    try { let first = CInt8(200); } catch [one: Error, ...oneRest] { let first_message = one.message; }
    try { let second = CInt8(300); } catch [two: Error, ...twoRest] { let second_message = two.message; }
    return value;
}
"#;
        for (target, process_limit, operation_limit) in
            [("expanded", 128, 128), ("observed", 100, 128)]
        {
            let source = format!(
                "{functions}\nasync fn main() {{ let original = Packet(text: \"日本語\\u{{0}}🙂\",bytes:b\"A\\xff\",high:18446744073709551615); let waiting = spawn {target}(original); let value = await waiting; }}"
            );
            let artifact = compile_source(&SourceFile::new(
                FileId(1),
                "worker-composite-budget.rinzu",
                source.clone(),
            ))
            .unwrap();
            let admission =
                SpawnAdmissionController::from_target_profile(rinzu_runtime::spawn::SpawnLimits {
                    worker_domains: 1,
                    in_flight_per_origin: 1,
                    process_transport_bytes: process_limit,
                    operation_transport_bytes: operation_limit,
                });
            let run = run_source_async_report_admitted(
                &artifact.ir,
                "main",
                SourceAsyncOptions {
                    gc_stress: true,
                    record_transport_evidence: true,
                    ..Default::default()
                },
                admission,
            )
            .unwrap();
            let failure = run.failure_details.as_ref().unwrap();
            assert_eq!(failure.primary.cause, FailureCause::Overloaded);
            assert_eq!(
                failure.primary.source.unwrap().start_byte as usize,
                source.find("spawn ").unwrap()
            );
            assert!(run.resources_returned && run.caught_errors.is_empty());
            assert_eq!(run.worker_heaps.len(), 1);
            assert_eq!(
                (
                    run.worker_heaps[0].heap.objects,
                    run.worker_heaps[0].heap.roots
                ),
                (0, 0)
            );
            assert!(
                run.worker_heaps[0].heap.peak_objects >= 4
                    && run.worker_heaps[0].heap.collections > 0
            );
            assert!(
                run.transport_evidence
                    .iter()
                    .all(|credit| credit.released_at.is_some())
            );
            if target == "expanded" {
                assert!(
                    run.transport_evidence
                        .iter()
                        .all(|credit| credit.direction == TransportDirection::Input)
                );
            } else {
                assert!(
                    run.transport_evidence
                        .iter()
                        .filter(|credit| credit.direction == TransportDirection::Result)
                        .count()
                        >= 2
                );
            }
        }
    }

    #[test]
    fn rejected_ninth_composite_spawn_never_starts_and_dropped_admitted_children_all_join() {
        let source = r#"
class Packet { const text: String, const bytes: Bytes(4), const high: CSize }
fn echo(value: Packet) -> Packet { return value; }
async fn main() {
    let original=Packet(text:"retained",bytes:b"A\xff",high:18446744073709551615);
    let one=spawn echo(original); let two=spawn echo(original); let three=spawn echo(original); let four=spawn echo(original);
    let five=spawn echo(original); let six=spawn echo(original); let seven=spawn echo(original); let eight=spawn echo(original);
    let denied=spawn echo(original);
    try { let unexpected=await denied; } catch [error: Error, ...errorRest] { let message=error.message; }
}
"#;
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "worker-composite-admission.rinzu",
            source,
        ))
        .unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.future_admissions.len(), 8);
        assert!(run.resources_returned, "{run:?}");
        assert_eq!(
            run.failure_details.as_ref().unwrap().primary.cause,
            FailureCause::Overloaded
        );
        // Scope failure can cancel queued Workers before their heap/body entry.
        // All eight accepted operations must still deliver physical completion.
        assert_eq!(run.worker_returns, 8);
        assert!(run.worker_heaps.len() <= 8);
        assert!(run.caught_errors.is_empty());
        assert_eq!(run.transport.peak_worker_domains, 8);
        assert!(
            run.worker_heaps
                .iter()
                .all(|worker| worker.heap.objects == 0 && worker.heap.roots == 0)
        );
        assert!(
            run.transport_evidence
                .iter()
                .all(|credit| credit.operation != Some(8) && credit.released_at.is_some())
        );
    }

    #[test]
    fn source_workers_prepare_all_composites_and_return_copies_before_origin_settlement() {
        let source = include_str!("../tests/fixtures/worker_composite.rinzu");
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "worker-composite.rinzu",
            source,
        ))
        .unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run.failure_details.is_none(), "{run:?}");
        assert!(run.resources_returned);
        assert_eq!(run.worker_heaps.len(), 9);
        assert!(run.worker_heaps.iter().all(|worker| worker.domain != 0
            && worker.heap.objects == 0
            && worker.heap.roots == 0
            && worker.heap.collections > 50
            && worker.heap.moved_objects > 50
            && worker.heap.peak_frames >= 2));
        assert!(run.heap.collections > 100 && run.heap.moved_objects > 100);
        assert_eq!(run.caught_errors.len(), 1);
        assert_eq!(
            run.caught_errors[0].primary.source.unwrap().start_byte as usize,
            source.find("CInt8(200)").unwrap()
        );
        assert_eq!(
            run.caught_errors[0].cleanup[0].source.unwrap().start_byte as usize,
            source.find("CInt8(300)").unwrap()
        );
        for input in run
            .transport_evidence
            .iter()
            .filter(|credit| credit.direction == TransportDirection::Input)
        {
            let released = input.released_at.unwrap();
            for result in run.transport_evidence.iter().filter(|credit| {
                credit.operation == input.operation
                    && credit.direction == TransportDirection::Result
            }) {
                assert!(released < result.reserved_at && result.released_at.is_some());
            }
        }
        assert!(
            run.events
                .iter()
                .filter_map(|event| {
                    if let SourceAsyncEvent::WorkerCompleted { worker_thread, .. } = event {
                        Some(worker_thread)
                    } else {
                        None
                    }
                })
                .all(|thread| *thread != run.owner_thread)
        );
    }

    #[test]
    fn cancelled_worker_composites_handle_local_cleanup_and_reclaim_the_actual_heap() {
        let source = include_str!("../tests/fixtures/worker_composite.rinzu")
            .replace("async fn main()", "async fn successful_main()");
        let source = format!(
            "{source}\n{}",
            include_str!("../tests/fixtures/worker_composite_cancel.rinzu")
        );
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "worker-composite-cancel.rinzu",
            source.clone(),
        ))
        .unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                cancel_worker_after_polls: Some(100),
                record_transport_evidence: true,
                ..Default::default()
            },
        )
        .unwrap();
        let failure = run.failure_details.as_ref().unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Cancelled);
        assert_eq!(
            failure.cleanup.iter().map(|d| d.cause).collect::<Vec<_>>(),
            [FailureCause::Error(ErrorKind::Conversion)]
        );
        assert_eq!(
            failure
                .cleanup
                .iter()
                .map(|d| d.source.unwrap().start_byte as usize)
                .collect::<Vec<_>>(),
            [source.find("CInt8(401)").unwrap()]
        );
        assert_eq!(run.caught_errors.len(), 1);
        assert_eq!(
            run.caught_errors[0].primary.source.unwrap().start_byte as usize,
            source.find("CInt8(301)").unwrap()
        );
        assert!(run.resources_returned && run.heap.objects == 0 && run.heap.roots == 0);
        assert_eq!(run.worker_heaps.len(), 1);
        let heap = run.worker_heaps[0].heap;
        assert!(
            heap.collections > 5 && heap.moved_objects > 5 && heap.objects == 0 && heap.roots == 0
        );
        assert!(
            run.transport_evidence
                .iter()
                .all(|credit| credit.released_at.is_some())
        );
    }
}

#[cfg(test)]
mod composite_callee_tests {
    use super::*;
    use crate::test_library::compile_library as compile_source;
    use rinzu_compiler::{FileId, SourceFile};

    #[test]
    fn parked_prepared_and_completed_composite_callees_keep_their_owner_roots() {
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "async-composite.rinzu",
            include_str!("../tests/fixtures/async_composite.rinzu"),
        ))
        .unwrap();
        let run = run_source_async(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(run.result, 0);
        assert!(run.resources_returned);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert!(run.heap.peak_frames >= 4, "{run:?}");
        assert!(
            run.heap.collections > 100 && run.heap.moved_objects > 100,
            "{run:?}"
        );
        assert!(run.heap.peak_objects < 70, "{run:?}");
        assert_eq!(run.awaited_scalars.len(), 2);
    }
    #[test]
    fn cancelled_composite_callees_keep_cleanup_roots_and_return_every_object() {
        let source = include_str!("../tests/fixtures/async_composite.rinzu")
            .replace("async fn main()", "async fn successful_main()");
        let source = format!(
            "{source}\n{}",
            include_str!("../tests/fixtures/async_composite_cancel.rinzu")
        );
        let artifact = compile_source(&SourceFile::new(
            FileId(1),
            "async-composite-cancel.rinzu",
            source,
        ))
        .unwrap();
        let run = run_source_async_report(
            &artifact.ir,
            "main",
            SourceAsyncOptions {
                gc_stress: true,
                cancel_after_polls: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        let failure = run.failure_details.unwrap();
        assert_eq!(failure.primary.cause, FailureCause::Cancelled);
        assert_eq!(
            failure.cleanup.iter().map(|d| d.cause).collect::<Vec<_>>(),
            [FailureCause::Error(ErrorKind::Conversion); 2]
        );
        assert!(run.resources_returned);
        assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
        assert!(run.heap.moved_objects > 0);
    }
}
