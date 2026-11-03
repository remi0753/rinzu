//! Process-main-thread standalone runtime pump.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, ThreadId};

use crate::resources::{
    ResourceError, ResourceKind, ResourceLedger, ResourceLimits, ResourceSnapshot,
};
use crate::scheduler::{
    ControlItem, ControlKey, EnqueueResult, ManagedQueue, OperationId, OperationResolver,
    Resolution, ResolutionEvent, ResolveResult, ScopeId, ScopePhase, TaskId, TaskOutcome,
    TaskState, TaskTable, TaskTableError, WakeGate, WorkItem,
};
use crate::{Runtime, RuntimeError};

/// Normalize default Ctrl-C before standalone execution starts runtime threads.
/// Embedded users do not call this: their host owns process interrupt policy.
pub fn initialize_interrupt() {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn rinzu_standalone_interrupt_startup();
        }
        // SAFETY: this standalone startup operation installs SIG_DFL and
        // unblocks SIGINT on its caller, without managed execution or allocation.
        unsafe { rinzu_standalone_interrupt_startup() };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandaloneOptions {
    pub resources: ResourceLimits,
    pub turn_limit: usize,
    pub cancellation_batch: usize,
}

impl Default for StandaloneOptions {
    fn default() -> Self {
        Self {
            resources: ResourceLimits::default(),
            turn_limit: 64,
            cancellation_batch: 64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskEvent {
    Start,
    Yielded,
    ChildSpawned(TaskId),
    Operation(Resolution),
    CancelRequested,
    ShutdownRequested { exit_code: i32 },
}

pub enum TaskAction {
    Return,
    Fail(String),
    Cancelled,
    Yield,
    Pending { label: String, cleanup_safe: bool },
    Spawn(Box<dyn ManagedTask>),
    RequestExit(i32),
}

impl fmt::Debug for TaskAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Return => formatter.write_str("Return"),
            Self::Fail(error) => formatter.debug_tuple("Fail").field(error).finish(),
            Self::Cancelled => formatter.write_str("Cancelled"),
            Self::Yield => formatter.write_str("Yield"),
            Self::Pending {
                label,
                cleanup_safe,
            } => formatter
                .debug_struct("Pending")
                .field("label", label)
                .field("cleanup_safe", cleanup_safe)
                .finish(),
            Self::Spawn(_) => formatter.write_str("Spawn(<managed task>)"),
            Self::RequestExit(code) => formatter.debug_tuple("RequestExit").field(code).finish(),
        }
    }
}

pub trait ManagedTask {
    fn resume(&mut self, event: TaskEvent) -> TaskAction;
}

#[derive(Clone, Debug)]
pub struct OperationTicket {
    pub operation: OperationId,
    pub task: TaskId,
    pub label: String,
    resolver: Arc<OperationResolver>,
    late_completions: Arc<AtomicUsize>,
    #[cfg(unix)]
    readiness: Option<crate::ready_work::ReadyPublication>,
    #[cfg(unix)]
    native_cancellation: crate::native_diagnostics::NativeCancellation,
}

impl OperationTicket {
    /// Bind on the physical Worker before entry and keep registration through
    /// its cleanup. Origin requests cannot see managed Worker state.
    #[cfg(unix)]
    pub fn observe_native_cancellation(
        &self,
        monitor: crate::native_diagnostics::ActivityMonitor,
    ) -> crate::native_diagnostics::NativeCancellationRegistration {
        self.native_cancellation.register(monitor)
    }
    /// Attach before submission. Cancellation/resolver completion do not
    /// publish physical readiness; only the helper's actual packet does.
    #[cfg(unix)]
    pub fn set_readiness(&mut self, publication: crate::ready_work::ReadyPublication) {
        if self.readiness.is_some() {
            // SAFETY: duplicate native observation is an invalid lifecycle.
            unsafe { libc::_exit(86) };
        }
        self.readiness = Some(publication);
    }
    pub(crate) fn publish_readiness(&self) {
        #[cfg(unix)]
        if let Some(publication) = &self.readiness {
            publication.publish();
        }
    }
    pub fn complete(&self, result: Result<(), String>) -> ResolveResult {
        let result = self.resolver.complete(result);
        if result == ResolveResult::Late {
            self.late_completions.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    pub fn cancel(&self) -> ResolveResult {
        let result = self.resolver.cancel();
        #[cfg(unix)]
        if result == ResolveResult::Won {
            self.native_cancellation.request();
        }
        result
    }

    pub fn timeout(&self) -> ResolveResult {
        let result = self.resolver.timeout();
        #[cfg(unix)]
        if result == ResolveResult::Won {
            self.native_cancellation.request();
        }
        result
    }
}

/// Child operations can start during a synchronous async prefix without
/// resolving the caller's pending await. The owner receives the terminal
/// winner, finishes native/Worker cleanup, and then explicitly settles its
/// pre-reserved operation and delivery credits.
pub struct OperationMailbox {
    owner: ThreadId,
    next_operation: u64,
    sender: Sender<ResolutionEvent>,
    receiver: Receiver<ResolutionEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    operations: HashMap<OperationId, Arc<OperationResolver>>,
    resolutions: HashMap<OperationId, Resolution>,
    ledger: ResourceLedger,
    late_completions: Arc<AtomicUsize>,
}

impl OperationMailbox {
    #[must_use]
    pub fn new(limits: ResourceLimits, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            owner: thread::current().id(),
            next_operation: 1,
            sender,
            receiver,
            wake,
            operations: HashMap::new(),
            resolutions: HashMap::new(),
            ledger: ResourceLedger::new(limits),
            late_completions: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn owner(&self) -> Result<(), StandaloneError> {
        let actual = thread::current().id();
        if self.owner != actual {
            return Err(RuntimeError::WrongThread {
                expected: self.owner,
                actual,
            }
            .into());
        }
        Ok(())
    }

    pub fn admit(
        &mut self,
        task: TaskId,
        label: String,
    ) -> Result<OperationTicket, StandaloneError> {
        self.owner()?;
        let operation = OperationId(self.next_operation);
        let next = self
            .next_operation
            .checked_add(1)
            .ok_or(StandaloneError::InvalidOperation(operation))?;
        self.ledger.reserve(&[
            (ResourceKind::PendingOperation, 1),
            (ResourceKind::QueueSlot, 1),
        ])?;
        self.next_operation = next;
        let resolver = Arc::new(OperationResolver::new(
            operation,
            self.sender.clone(),
            Arc::clone(&self.wake),
        ));
        self.operations.insert(operation, Arc::clone(&resolver));
        Ok(OperationTicket {
            operation,
            task,
            label,
            resolver,
            late_completions: Arc::clone(&self.late_completions),
            #[cfg(unix)]
            readiness: None,
            #[cfg(unix)]
            native_cancellation: Default::default(),
        })
    }

    /// A cancel winner can arrive before the Worker physically returns. Import
    /// does not release credits or acknowledge owner-side materialization.
    pub fn take_resolutions(&mut self) -> Result<Vec<ResolutionEvent>, StandaloneError> {
        self.owner()?;
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            if !self.operations.contains_key(&event.operation)
                || self
                    .resolutions
                    .insert(event.operation, event.resolution.clone())
                    .is_some()
            {
                return Err(StandaloneError::InvalidOperation(event.operation));
            }
            events.push(event);
        }
        Ok(events)
    }

    pub fn settle(&mut self, operation: OperationId) -> Result<Resolution, StandaloneError> {
        self.owner()?;
        let resolution = self
            .resolutions
            .remove(&operation)
            .ok_or(StandaloneError::InvalidOperation(operation))?;
        let resolver = self
            .operations
            .remove(&operation)
            .ok_or(StandaloneError::InvalidOperation(operation))?;
        if !resolver.mark_settled() {
            return Err(StandaloneError::InvalidOperation(operation));
        }
        self.ledger.release(&[
            (ResourceKind::PendingOperation, 1),
            (ResourceKind::QueueSlot, 1),
        ])?;
        Ok(resolution)
    }

    #[must_use]
    pub fn resources(&self) -> ResourceSnapshot {
        self.ledger.snapshot()
    }

    #[must_use]
    pub fn late_completions(&self) -> usize {
        self.late_completions.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunStatus {
    Stalled { pending_operations: usize },
    Complete(TaskOutcome),
    ShutdownComplete(ShutdownReport),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownPhase {
    Requested,
    Unwinding,
    InitiatorSettled,
    RootClosing,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownUnwind {
    pub exit_code: i32,
    pub initiator: TaskId,
    pub phase: ShutdownPhase,
    pub initiator_outcome: Option<TaskOutcome>,
    pub later_exit_requests: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeBlocker {
    pub task: TaskId,
    pub source: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownReport {
    pub exit_code: i32,
    pub initiator: TaskId,
    pub initiator_outcome: TaskOutcome,
    pub later_exit_requests: usize,
    pub late_completions: usize,
    pub native_blockers: Vec<NativeBlocker>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnReport {
    pub processed: usize,
    pub request_follow_up_wake: bool,
}

#[derive(Debug)]
pub enum StandaloneError {
    Runtime(RuntimeError),
    Resource(ResourceError),
    Task(TaskTableError),
    NoMain,
    MainAlreadyStarted,
    InvalidOperation(OperationId),
    InvalidTaskAction {
        task: TaskId,
        action: &'static str,
    },
    AdmissionClosed {
        task: TaskId,
        operation: &'static str,
    },
}

impl fmt::Display for StandaloneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(error) => error.fmt(formatter),
            Self::Resource(error) => error.fmt(formatter),
            Self::Task(error) => error.fmt(formatter),
            Self::NoMain => formatter.write_str("standalone runtime has no main task"),
            Self::MainAlreadyStarted => formatter.write_str("main task was already started"),
            Self::InvalidOperation(operation) => {
                write!(formatter, "unknown operation {operation:?}")
            }
            Self::InvalidTaskAction { task, action } => {
                write!(
                    formatter,
                    "task {task:?} cannot {action} in its current state"
                )
            }
            Self::AdmissionClosed { task, operation } => write!(
                formatter,
                "shutdown closed {operation} admission for task {task:?}"
            ),
        }
    }
}

impl std::error::Error for StandaloneError {}

impl From<RuntimeError> for StandaloneError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

impl From<ResourceError> for StandaloneError {
    fn from(value: ResourceError) -> Self {
        Self::Resource(value)
    }
}

impl From<TaskTableError> for StandaloneError {
    fn from(value: TaskTableError) -> Self {
        Self::Task(value)
    }
}

struct RuntimeTask {
    program: Option<Box<dyn ManagedTask>>,
    next_event: Option<TaskEvent>,
    cancellation_delivered: bool,
    closing_outcome: Option<TaskOutcome>,
    unobserved_child_failure: Option<String>,
}

struct OperationRecord {
    task: TaskId,
    resolver: Arc<OperationResolver>,
    cleanup_safe: bool,
}

/// The v0.1 standalone owner pump. All methods that execute `ManagedTask`
/// code enforce the `Runtime` owner-thread activation guard.
pub struct StandaloneRuntime {
    core: Arc<Runtime>,
    queue: ManagedQueue,
    wake_gate: Arc<WakeGate>,
    tasks: TaskTable,
    task_programs: HashMap<TaskId, RuntimeTask>,
    operations: HashMap<OperationId, OperationRecord>,
    resolution_events: HashMap<OperationId, Resolution>,
    resolution_receiver: Receiver<ResolutionEvent>,
    resolution_sender: mpsc::Sender<ResolutionEvent>,
    new_tickets: VecDeque<OperationTicket>,
    ledger: ResourceLedger,
    cancellation_batch: usize,
    next_operation: u64,
    main_task: Option<TaskId>,
    root_outcome: Option<TaskOutcome>,
    shutdown: Option<ShutdownUnwind>,
    late_completions: Arc<AtomicUsize>,
    native_blockers: Vec<NativeBlocker>,
}

impl StandaloneRuntime {
    #[must_use]
    pub fn new(options: StandaloneOptions) -> Self {
        assert!(options.turn_limit > 0, "turn_limit must be non-zero");
        assert!(
            options.cancellation_batch > 0,
            "cancellation_batch must be non-zero"
        );
        assert!(
            options.resources.control_slots > 0,
            "at least one control slot must be reserved"
        );
        let core = Arc::new(Runtime::new());
        let wake_gate = Arc::new(WakeGate::new());
        let (resolution_sender, resolution_receiver) = mpsc::channel();
        Self {
            core,
            queue: ManagedQueue::new(options.turn_limit, Arc::clone(&wake_gate)),
            wake_gate,
            tasks: TaskTable::new(),
            task_programs: HashMap::new(),
            operations: HashMap::new(),
            resolution_events: HashMap::new(),
            resolution_receiver,
            resolution_sender,
            new_tickets: VecDeque::new(),
            ledger: ResourceLedger::new(options.resources),
            cancellation_batch: options.cancellation_batch,
            next_operation: 0,
            main_task: None,
            root_outcome: None,
            shutdown: None,
            late_completions: Arc::new(AtomicUsize::new(0)),
            native_blockers: Vec::new(),
        }
    }

    pub fn start_main(&mut self, program: Box<dyn ManagedTask>) -> Result<TaskId, StandaloneError> {
        if self.main_task.is_some() {
            return Err(StandaloneError::MainAlreadyStarted);
        }
        let task = self.admit_task(self.tasks.root_scope(), program)?;
        self.main_task = Some(task);
        Ok(task)
    }

    pub fn cancel_task(&mut self, task: TaskId) -> Result<(), StandaloneError> {
        if !self.tasks.request_cancel(task)? {
            return Ok(());
        }
        match self.tasks.state(task)?.clone() {
            TaskState::Pending(operation) => {
                let operation = self
                    .operations
                    .get(&operation)
                    .ok_or(StandaloneError::InvalidOperation(operation))?;
                let _ = operation.resolver.cancel();
            }
            TaskState::Runnable => {
                self.enqueue_work(WorkItem::Task(task));
            }
            TaskState::Running | TaskState::Closing(_) | TaskState::Settled(_) => {}
        }
        Ok(())
    }

    pub fn mark_native_blocker(
        &mut self,
        task: TaskId,
        source: impl Into<String>,
    ) -> Result<(), StandaloneError> {
        let _ = self.tasks.state(task)?;
        let source = source.into();
        if !self
            .native_blockers
            .iter()
            .any(|blocker| blocker.task == task && blocker.source == source)
        {
            self.native_blockers.push(NativeBlocker { task, source });
        }
        Ok(())
    }

    pub fn clear_native_blocker(&mut self, task: TaskId, source: &str) {
        self.native_blockers
            .retain(|blocker| blocker.task != task || blocker.source != source);
    }

    pub fn pump_turn(&mut self) -> Result<TurnReport, StandaloneError> {
        if self.main_task.is_none() {
            return Err(StandaloneError::NoMain);
        }
        self.import_resolutions();
        let core = Arc::clone(&self.core);
        let _activation = core.enter_managed()?;
        self.queue.begin_turn();
        let mut processed = 0;
        while processed < self.queue.turn_limit() {
            let Some(item) = self.queue.pop_next() else {
                break;
            };
            processed += 1;
            match item {
                crate::scheduler::QueueItem::Work(WorkItem::Task(task)) => {
                    self.run_task(task)?;
                }
                crate::scheduler::QueueItem::Work(WorkItem::Operation(operation)) => {
                    self.finish_operation(operation)?;
                }
                crate::scheduler::QueueItem::Control(control) => {
                    self.run_control(control)?;
                }
            }
        }
        let request_follow_up_wake = self.queue.finish_turn();
        Ok(TurnReport {
            processed,
            request_follow_up_wake,
        })
    }

    pub fn run_until_stalled(&mut self) -> Result<RunStatus, StandaloneError> {
        loop {
            let report = self.pump_turn()?;
            if matches!(
                self.shutdown.as_ref().map(|shutdown| shutdown.phase),
                Some(ShutdownPhase::Complete)
            ) {
                return Ok(RunStatus::ShutdownComplete(
                    self.shutdown_report()
                        .expect("complete shutdown must have a report"),
                ));
            }
            if self.shutdown.is_none()
                && let Some(outcome) = &self.root_outcome
            {
                return Ok(RunStatus::Complete(outcome.clone()));
            }
            if report.processed == 0 {
                self.import_resolutions();
                if self.queue.is_empty() {
                    return Ok(RunStatus::Stalled {
                        pending_operations: self.operations.len(),
                    });
                }
            }
        }
    }

    #[must_use]
    pub fn take_operation_tickets(&mut self) -> Vec<OperationTicket> {
        self.new_tickets.drain(..).collect()
    }

    #[must_use]
    pub fn resources(&self) -> ResourceSnapshot {
        self.ledger.snapshot()
    }

    #[must_use]
    pub fn root_outcome(&self) -> Option<&TaskOutcome> {
        self.root_outcome.as_ref()
    }

    #[must_use]
    pub fn main_task(&self) -> Option<TaskId> {
        self.main_task
    }

    #[must_use]
    pub fn shutdown_unwind(&self) -> Option<&ShutdownUnwind> {
        self.shutdown.as_ref()
    }

    #[must_use]
    pub fn shutdown_report(&self) -> Option<ShutdownReport> {
        let shutdown = self.shutdown.as_ref()?;
        (shutdown.phase == ShutdownPhase::Complete).then(|| ShutdownReport {
            exit_code: shutdown.exit_code,
            initiator: shutdown.initiator,
            initiator_outcome: shutdown
                .initiator_outcome
                .clone()
                .expect("a completed shutdown has a settled initiator"),
            later_exit_requests: shutdown.later_exit_requests,
            late_completions: self.late_completions.load(Ordering::Relaxed),
            native_blockers: self.native_blockers.clone(),
        })
    }

    fn admit_task(
        &mut self,
        parent_scope: ScopeId,
        program: Box<dyn ManagedTask>,
    ) -> Result<TaskId, StandaloneError> {
        self.ledger
            .reserve(&[(ResourceKind::LiveTask, 1), (ResourceKind::QueueSlot, 1)])?;
        let task = match self.tasks.insert_task(parent_scope) {
            Ok(task) => task,
            Err(error) => {
                self.ledger
                    .release(&[(ResourceKind::LiveTask, 1), (ResourceKind::QueueSlot, 1)])?;
                return Err(error.into());
            }
        };
        self.task_programs.insert(
            task,
            RuntimeTask {
                program: Some(program),
                next_event: Some(TaskEvent::Start),
                cancellation_delivered: false,
                closing_outcome: None,
                unobserved_child_failure: None,
            },
        );
        self.enqueue_work(WorkItem::Task(task));
        Ok(task)
    }

    fn run_task(&mut self, task: TaskId) -> Result<(), StandaloneError> {
        if !matches!(self.tasks.state(task)?, TaskState::Runnable) {
            return Ok(());
        }
        self.tasks.set_running(task)?;
        let cancelled = self.tasks.cancel_requested(task)?;
        let record =
            self.task_programs
                .get_mut(&task)
                .ok_or(StandaloneError::InvalidTaskAction {
                    task,
                    action: "resume after settlement",
                })?;
        let event = if cancelled && !record.cancellation_delivered {
            // The request stays set for managed polls, but its notification
            // must not replace the completion of every cleanup operation.
            record.cancellation_delivered = true;
            record.next_event.take();
            TaskEvent::CancelRequested
        } else {
            record.next_event.take().unwrap_or(TaskEvent::Yielded)
        };
        let mut program = record
            .program
            .take()
            .expect("a runnable task must retain its program");
        let action = program.resume(event);
        self.task_programs
            .get_mut(&task)
            .expect("running task record must remain live")
            .program = Some(program);
        self.apply_action(task, action)
    }

    fn apply_action(&mut self, task: TaskId, action: TaskAction) -> Result<(), StandaloneError> {
        match action {
            TaskAction::Return => self.begin_task_close(task, TaskOutcome::Returned),
            TaskAction::Fail(error) => self.begin_task_close(task, TaskOutcome::Failed(error)),
            TaskAction::Cancelled => self.begin_task_close(task, TaskOutcome::Cancelled),
            TaskAction::Yield => {
                self.tasks.yield_running(task)?;
                self.task_programs
                    .get_mut(&task)
                    .expect("running task record must remain live")
                    .next_event = Some(TaskEvent::Yielded);
                self.enqueue_work(WorkItem::Task(task));
                Ok(())
            }
            TaskAction::Pending {
                label,
                cleanup_safe,
            } => {
                if !self.operation_admission_allowed(task, cleanup_safe) {
                    return self.begin_task_close(
                        task,
                        TaskOutcome::Failed("ShutdownAdmissionClosed: operation".into()),
                    );
                }
                self.admit_operation(task, label, cleanup_safe)
            }
            TaskAction::Spawn(program) => {
                if self.shutdown.is_some() {
                    return self.begin_task_close(
                        task,
                        TaskOutcome::Failed("ShutdownAdmissionClosed: spawn".into()),
                    );
                }
                let scope = self.tasks.child_scope(task)?;
                let child = self.admit_task(scope, program)?;
                self.tasks.yield_running(task)?;
                self.task_programs
                    .get_mut(&task)
                    .expect("running task record must remain live")
                    .next_event = Some(TaskEvent::ChildSpawned(child));
                self.enqueue_work(WorkItem::Task(task));
                Ok(())
            }
            TaskAction::RequestExit(exit_code) => self.request_exit(task, exit_code),
        }
    }

    fn operation_admission_allowed(&self, task: TaskId, cleanup_safe: bool) -> bool {
        match &self.shutdown {
            None => true,
            Some(shutdown) => {
                cleanup_safe
                    && shutdown.initiator == task
                    && matches!(
                        shutdown.phase,
                        ShutdownPhase::Requested | ShutdownPhase::Unwinding
                    )
            }
        }
    }

    fn admit_operation(
        &mut self,
        task: TaskId,
        label: String,
        cleanup_safe: bool,
    ) -> Result<(), StandaloneError> {
        self.ledger.reserve(&[
            (ResourceKind::PendingOperation, 1),
            (ResourceKind::QueueSlot, 1),
        ])?;
        let operation = OperationId(self.next_operation);
        self.next_operation += 1;
        let core = Arc::downgrade(&self.core);
        let gate = Arc::clone(&self.wake_gate);
        let resolver = Arc::new(OperationResolver::new(
            operation,
            self.resolution_sender.clone(),
            Arc::new(move || {
                if gate.request()
                    && let Some(core) = core.upgrade()
                {
                    core.request_wake();
                }
            }),
        ));
        self.operations.insert(
            operation,
            OperationRecord {
                task,
                resolver: Arc::clone(&resolver),
                cleanup_safe,
            },
        );
        self.tasks.set_pending(task, operation)?;
        self.new_tickets.push_back(OperationTicket {
            operation,
            task,
            label,
            resolver,
            late_completions: Arc::clone(&self.late_completions),
            #[cfg(unix)]
            readiness: None,
            #[cfg(unix)]
            native_cancellation: Default::default(),
        });
        Ok(())
    }

    fn import_resolutions(&mut self) {
        while let Ok(event) = self.resolution_receiver.try_recv() {
            if self.operations.contains_key(&event.operation) {
                self.enqueue_work(WorkItem::Operation(event.operation));
                // The resolver guarantees one event. Store it beside the operation
                // until its pre-reserved queue slot reaches the owner thread.
                self.resolution_events
                    .insert(event.operation, event.resolution);
            }
        }
    }

    fn finish_operation(&mut self, operation: OperationId) -> Result<(), StandaloneError> {
        let resolution = self
            .resolution_events
            .remove(&operation)
            .ok_or(StandaloneError::InvalidOperation(operation))?;
        let record = self
            .operations
            .remove(&operation)
            .ok_or(StandaloneError::InvalidOperation(operation))?;
        let _ = record.cleanup_safe;
        if !record.resolver.mark_settled() {
            return Err(StandaloneError::InvalidOperation(operation));
        }
        self.ledger.release(&[
            (ResourceKind::PendingOperation, 1),
            (ResourceKind::QueueSlot, 1),
        ])?;
        if matches!(self.tasks.state(record.task)?, TaskState::Pending(id) if *id == operation) {
            self.tasks.set_runnable(record.task)?;
            self.task_programs
                .get_mut(&record.task)
                .expect("pending task record must remain live")
                .next_event = Some(TaskEvent::Operation(resolution));
            self.enqueue_work(WorkItem::Task(record.task));
        }
        Ok(())
    }

    fn request_exit(&mut self, task: TaskId, exit_code: i32) -> Result<(), StandaloneError> {
        if let Some(shutdown) = self.shutdown.as_mut() {
            shutdown.later_exit_requests += 1;
            return self.begin_task_close(task, TaskOutcome::Cancelled);
        }

        self.shutdown = Some(ShutdownUnwind {
            exit_code,
            initiator: task,
            phase: ShutdownPhase::Requested,
            initiator_outcome: None,
            later_exit_requests: 0,
        });
        self.tasks.yield_running(task)?;
        self.task_programs
            .get_mut(&task)
            .expect("shutdown initiator must remain live")
            .next_event = Some(TaskEvent::ShutdownRequested { exit_code });
        self.schedule_shutdown_control()
    }

    fn begin_task_close(
        &mut self,
        task: TaskId,
        mut outcome: TaskOutcome,
    ) -> Result<(), StandaloneError> {
        if matches!(outcome, TaskOutcome::Returned)
            && let Some(error) = self
                .task_programs
                .get_mut(&task)
                .expect("running task record must remain live")
                .unobserved_child_failure
                .take()
        {
            outcome = TaskOutcome::Failed(format!("UnobservedChildFailure: {error}"));
        }
        let phase = match outcome {
            TaskOutcome::Returned => ScopePhase::ClosingNormal,
            TaskOutcome::Failed(_) => ScopePhase::ClosingFailure,
            TaskOutcome::Cancelled => ScopePhase::ClosingCancelled,
        };
        self.tasks.begin_close(task, phase)?;
        let record = self
            .task_programs
            .get_mut(&task)
            .expect("running task record must remain live");
        record.program = None;
        record.closing_outcome = Some(outcome);

        let scope = self.tasks.child_scope(task)?;
        let shutdown_initiator = self
            .shutdown
            .as_ref()
            .is_some_and(|shutdown| shutdown.initiator == task);
        if (matches!(
            phase,
            ScopePhase::ClosingFailure | ScopePhase::ClosingCancelled
        ) || shutdown_initiator)
            && self.tasks.child_count(scope)? > 0
        {
            self.schedule_scope_cancellation(scope)?;
        }
        self.finalize_ready_task(task)
    }

    fn finalize_ready_task(&mut self, task: TaskId) -> Result<(), StandaloneError> {
        if self.tasks.child_count(self.tasks.child_scope(task)?)? != 0 {
            return Ok(());
        }
        let mut outcome = self
            .task_programs
            .get_mut(&task)
            .and_then(|record| record.closing_outcome.take())
            .ok_or(StandaloneError::InvalidTaskAction {
                task,
                action: "settle before body completion",
            })?;
        if matches!(outcome, TaskOutcome::Returned)
            && let Some(error) = self
                .task_programs
                .get_mut(&task)
                .and_then(|record| record.unobserved_child_failure.take())
        {
            outcome = TaskOutcome::Failed(format!("UnobservedChildFailure: {error}"));
        }
        let parent_scope = self.tasks.settle(task, outcome.clone())?;
        self.task_programs.remove(&task);
        self.ledger
            .release(&[(ResourceKind::LiveTask, 1), (ResourceKind::QueueSlot, 1)])?;

        let settled_shutdown_initiator = self
            .shutdown
            .as_ref()
            .is_some_and(|shutdown| shutdown.initiator == task);
        if settled_shutdown_initiator {
            let shutdown = self
                .shutdown
                .as_mut()
                .expect("shutdown initiator requires an unwind coordinator");
            shutdown.initiator_outcome = Some(outcome.clone());
            shutdown.phase = ShutdownPhase::InitiatorSettled;
            self.schedule_shutdown_control()?;
        }

        if let TaskOutcome::Failed(error) = &outcome
            && let Some(parent) = self.tasks.scope_owner(parent_scope)?
            && let Some(parent_record) = self.task_programs.get_mut(&parent)
        {
            parent_record
                .unobserved_child_failure
                .get_or_insert_with(|| error.clone());
        }

        if let Some(parent) = self.tasks.scope_owner(parent_scope)? {
            if matches!(self.tasks.state(parent)?, TaskState::Closing(_)) {
                self.finalize_ready_task(parent)?;
            }
        } else if Some(task) == self.main_task {
            self.root_outcome = Some(outcome);
        }
        self.maybe_complete_shutdown()?;
        Ok(())
    }

    fn schedule_scope_cancellation(&mut self, scope: ScopeId) -> Result<(), StandaloneError> {
        self.ledger.reserve(&[(ResourceKind::ControlSlot, 1)])?;
        let result = self.queue.enqueue_control(ControlItem {
            key: ControlKey::CancelScope(scope),
            cursor: None,
        });
        match result {
            EnqueueResult::Queued { request_wake } => {
                if request_wake {
                    self.core.request_wake();
                }
            }
            EnqueueResult::Coalesced => {
                self.ledger.release(&[(ResourceKind::ControlSlot, 1)])?;
            }
        }
        Ok(())
    }

    fn schedule_shutdown_control(&mut self) -> Result<(), StandaloneError> {
        self.ledger.reserve(&[(ResourceKind::ControlSlot, 1)])?;
        let result = self.queue.enqueue_control(ControlItem {
            key: ControlKey::Shutdown,
            cursor: None,
        });
        match result {
            EnqueueResult::Queued { request_wake } => {
                if request_wake {
                    self.core.request_wake();
                }
            }
            EnqueueResult::Coalesced => {
                self.ledger.release(&[(ResourceKind::ControlSlot, 1)])?;
            }
        }
        Ok(())
    }

    fn run_control(&mut self, item: ControlItem) -> Result<(), StandaloneError> {
        match item.key {
            ControlKey::CancelScope(scope) => {
                let mut cursor = match item.cursor {
                    Some(cursor) => Some(cursor),
                    None => self.tasks.first_child(scope)?,
                };
                for _ in 0..self.cancellation_batch {
                    let Some(task) = cursor else {
                        break;
                    };
                    cursor = self.tasks.next_sibling(task)?;
                    self.cancel_task(task)?;
                }
                if let Some(cursor) = cursor {
                    let result = self.queue.enqueue_control(ControlItem {
                        key: item.key,
                        cursor: Some(cursor),
                    });
                    debug_assert!(matches!(result, EnqueueResult::Queued { .. }));
                } else {
                    self.ledger.release(&[(ResourceKind::ControlSlot, 1)])?;
                }
            }
            ControlKey::Shutdown => {
                let phase = self
                    .shutdown
                    .as_ref()
                    .map(|shutdown| shutdown.phase)
                    .expect("shutdown control requires an unwind coordinator");
                match phase {
                    ShutdownPhase::Requested => {
                        let initiator = {
                            let shutdown = self
                                .shutdown
                                .as_mut()
                                .expect("shutdown coordinator was checked above");
                            shutdown.phase = ShutdownPhase::Unwinding;
                            shutdown.initiator
                        };
                        self.enqueue_work(WorkItem::Task(initiator));
                        self.ledger.release(&[(ResourceKind::ControlSlot, 1)])?;
                    }
                    ShutdownPhase::InitiatorSettled | ShutdownPhase::RootClosing => {
                        self.shutdown
                            .as_mut()
                            .expect("shutdown coordinator was checked above")
                            .phase = ShutdownPhase::RootClosing;
                        let root = self.tasks.root_scope();
                        let mut cursor = match item.cursor {
                            Some(cursor) => Some(cursor),
                            None => self.tasks.first_child(root)?,
                        };
                        for _ in 0..self.cancellation_batch {
                            let Some(task) = cursor else {
                                break;
                            };
                            cursor = self.tasks.next_sibling(task)?;
                            self.cancel_task(task)?;
                        }
                        if let Some(cursor) = cursor {
                            let result = self.queue.enqueue_control(ControlItem {
                                key: item.key,
                                cursor: Some(cursor),
                            });
                            debug_assert!(matches!(result, EnqueueResult::Queued { .. }));
                        } else {
                            self.ledger.release(&[(ResourceKind::ControlSlot, 1)])?;
                            self.maybe_complete_shutdown()?;
                        }
                    }
                    ShutdownPhase::Unwinding | ShutdownPhase::Complete => {
                        self.ledger.release(&[(ResourceKind::ControlSlot, 1)])?;
                    }
                }
            }
        }
        Ok(())
    }

    fn maybe_complete_shutdown(&mut self) -> Result<(), StandaloneError> {
        let should_complete = self.shutdown.as_ref().is_some_and(|shutdown| {
            shutdown.phase == ShutdownPhase::RootClosing && shutdown.initiator_outcome.is_some()
        }) && self.tasks.child_count(self.tasks.root_scope())? == 0;
        if should_complete {
            self.shutdown
                .as_mut()
                .expect("completion requires a shutdown coordinator")
                .phase = ShutdownPhase::Complete;
        }
        Ok(())
    }

    fn enqueue_work(&mut self, item: WorkItem) {
        if let EnqueueResult::Queued { request_wake: true } = self.queue.enqueue_work(item) {
            self.core.request_wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Immediate(TaskAction);

    impl ManagedTask for Immediate {
        fn resume(&mut self, _event: TaskEvent) -> TaskAction {
            std::mem::replace(&mut self.0, TaskAction::Return)
        }
    }

    struct PendingOnce {
        thread_ids: Arc<Mutex<Vec<std::thread::ThreadId>>>,
    }

    impl ManagedTask for PendingOnce {
        fn resume(&mut self, event: TaskEvent) -> TaskAction {
            self.thread_ids
                .lock()
                .unwrap()
                .push(std::thread::current().id());
            match event {
                TaskEvent::Start => TaskAction::Pending {
                    label: "test future".into(),
                    cleanup_safe: false,
                },
                TaskEvent::Operation(Resolution::Completed(Ok(()))) => TaskAction::Return,
                TaskEvent::CancelRequested
                | TaskEvent::Operation(Resolution::Cancelled | Resolution::TimedOut) => {
                    TaskAction::Cancelled
                }
                other => TaskAction::Fail(format!("unexpected event: {other:?}")),
            }
        }
    }

    struct Parent {
        child: Option<Box<dyn ManagedTask>>,
    }

    impl ManagedTask for Parent {
        fn resume(&mut self, event: TaskEvent) -> TaskAction {
            match event {
                TaskEvent::Start => TaskAction::Spawn(self.child.take().unwrap()),
                TaskEvent::ChildSpawned(_) => TaskAction::Return,
                other => TaskAction::Fail(format!("unexpected event: {other:?}")),
            }
        }
    }

    #[test]
    fn main_future_and_parent_scope_settle_on_owner_thread() {
        let owner = std::thread::current().id();
        let thread_ids = Arc::new(Mutex::new(Vec::new()));
        let child = PendingOnce {
            thread_ids: Arc::clone(&thread_ids),
        };
        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        runtime
            .start_main(Box::new(Parent {
                child: Some(Box::new(child)),
            }))
            .unwrap();
        assert_eq!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Stalled {
                pending_operations: 1
            }
        );
        let ticket = runtime.take_operation_tickets().pop().unwrap();
        assert_eq!(ticket.complete(Ok(())), ResolveResult::Won);
        assert_eq!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Complete(TaskOutcome::Returned)
        );
        assert!(thread_ids.lock().unwrap().iter().all(|id| *id == owner));
        assert!(runtime.resources().used.is_baseline());
    }

    #[test]
    fn parent_failure_cancels_pending_child_and_propagates_failure() {
        struct FailingParent {
            child: Option<Box<dyn ManagedTask>>,
        }
        impl ManagedTask for FailingParent {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start => TaskAction::Spawn(self.child.take().unwrap()),
                    TaskEvent::ChildSpawned(_) => TaskAction::Fail("parent failed".into()),
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        runtime
            .start_main(Box::new(FailingParent {
                child: Some(Box::new(PendingOnce {
                    thread_ids: Arc::new(Mutex::new(Vec::new())),
                })),
            }))
            .unwrap();
        assert_eq!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Complete(TaskOutcome::Failed("parent failed".into()))
        );
        assert!(runtime.resources().used.is_baseline());
        let ticket = runtime.take_operation_tickets().pop().unwrap();
        assert_eq!(ticket.complete(Ok(())), ResolveResult::Late);
    }

    #[test]
    fn explicit_cancel_settles_main() {
        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        let main = runtime
            .start_main(Box::new(PendingOnce {
                thread_ids: Arc::new(Mutex::new(Vec::new())),
            }))
            .unwrap();
        assert!(matches!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Stalled { .. }
        ));
        runtime.cancel_task(main).unwrap();
        assert_eq!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Complete(TaskOutcome::Cancelled)
        );
        assert!(runtime.resources().used.is_baseline());
    }

    #[test]
    fn cancellation_notification_does_not_replace_cleanup_completions() {
        struct CleanupAfterCancel {
            completed: usize,
        }
        impl ManagedTask for CleanupAfterCancel {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start => TaskAction::Pending {
                        label: "body".into(),
                        cleanup_safe: false,
                    },
                    TaskEvent::CancelRequested => TaskAction::Pending {
                        label: "cleanup 1".into(),
                        cleanup_safe: true,
                    },
                    TaskEvent::Operation(Resolution::Completed(Ok(()))) => {
                        self.completed += 1;
                        if self.completed == 2 {
                            TaskAction::Return
                        } else {
                            TaskAction::Pending {
                                label: "cleanup 2".into(),
                                cleanup_safe: true,
                            }
                        }
                    }
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        let main = runtime
            .start_main(Box::new(CleanupAfterCancel { completed: 0 }))
            .unwrap();
        runtime.run_until_stalled().unwrap();
        let body = runtime.take_operation_tickets().pop().unwrap();
        runtime.cancel_task(main).unwrap();
        runtime.cancel_task(main).unwrap();
        runtime.run_until_stalled().unwrap();
        for label in ["cleanup 1", "cleanup 2"] {
            let ticket = runtime.take_operation_tickets().pop().unwrap();
            assert_eq!(ticket.label, label);
            assert_eq!(ticket.complete(Ok(())), ResolveResult::Won);
            // Keep the regression bounded even if a repeated cancellation
            // notification were to replace the completion again.
            for _ in 0..4 {
                runtime.pump_turn().unwrap();
            }
        }
        assert_eq!(runtime.root_outcome(), Some(&TaskOutcome::Returned));
        assert!(runtime.resources().used.is_baseline());
        assert_eq!(body.complete(Ok(())), ResolveResult::Late);
    }

    #[test]
    fn synchronous_main_returns_all_resources() {
        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        runtime
            .start_main(Box::new(Immediate(TaskAction::Return)))
            .unwrap();
        assert_eq!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Complete(TaskOutcome::Returned)
        );
        assert!(runtime.resources().used.is_baseline());
    }

    #[test]
    fn one_reserved_control_slot_cancels_a_saturated_scope_in_batches() {
        struct ManyChildren {
            remaining: usize,
        }
        impl ManagedTask for ManyChildren {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start | TaskEvent::ChildSpawned(_) if self.remaining > 0 => {
                        self.remaining -= 1;
                        TaskAction::Spawn(Box::new(PendingOnce {
                            thread_ids: Arc::new(Mutex::new(Vec::new())),
                        }))
                    }
                    TaskEvent::ChildSpawned(_) => TaskAction::Fail("stop all children".into()),
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let resources = ResourceLimits {
            live_tasks: 5,
            pending_operations: 4,
            registrations: 0,
            queue_slots: 9,
            control_slots: 1,
        };
        let mut runtime = StandaloneRuntime::new(StandaloneOptions {
            resources,
            turn_limit: 1,
            cancellation_batch: 1,
        });
        runtime
            .start_main(Box::new(ManyChildren { remaining: 4 }))
            .unwrap();

        let mut turns = 0;
        let outcome = loop {
            turns += 1;
            let report = runtime.pump_turn().unwrap();
            if let Some(outcome) = runtime.root_outcome() {
                break outcome.clone();
            }
            assert!(report.processed > 0 || turns < 32);
        };
        assert_eq!(outcome, TaskOutcome::Failed("stop all children".into()));
        assert!(turns > 4, "cursor cancellation must span bounded turns");
        let resources = runtime.resources();
        assert_eq!(resources.peak.queue_slots, resources.limits.queue_slots);
        assert_eq!(resources.peak.control_slots, 1);
        assert!(resources.used.is_baseline());
        assert!(
            runtime
                .take_operation_tickets()
                .iter()
                .all(|ticket| ticket.complete(Ok(())) == ResolveResult::Late)
        );
    }

    #[test]
    fn shutdown_unwind_allows_cleanup_safe_operation_and_reports_blockers() {
        struct CleanupOnExit;
        impl ManagedTask for CleanupOnExit {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start => TaskAction::RequestExit(23),
                    TaskEvent::ShutdownRequested { exit_code: 23 } => TaskAction::Pending {
                        label: "cleanup close".into(),
                        cleanup_safe: true,
                    },
                    TaskEvent::Operation(Resolution::Completed(Ok(()))) => TaskAction::Return,
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        let main = runtime.start_main(Box::new(CleanupOnExit)).unwrap();
        runtime
            .mark_native_blocker(main, "native call without cancel hook")
            .unwrap();
        assert_eq!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Stalled {
                pending_operations: 1
            }
        );
        let ticket = runtime.take_operation_tickets().pop().unwrap();
        assert_eq!(ticket.complete(Ok(())), ResolveResult::Won);
        let RunStatus::ShutdownComplete(report) = runtime.run_until_stalled().unwrap() else {
            panic!("exit request must complete through ShutdownUnwind");
        };
        assert_eq!(report.exit_code, 23);
        assert_eq!(report.initiator, main);
        assert_eq!(report.initiator_outcome, TaskOutcome::Returned);
        assert_eq!(report.native_blockers.len(), 1);
        assert_eq!(
            report.native_blockers[0].source,
            "native call without cancel hook"
        );
        assert!(runtime.resources().used.is_baseline());

        assert_eq!(ticket.complete(Ok(())), ResolveResult::Late);
        assert_eq!(runtime.shutdown_report().unwrap().late_completions, 1);
    }

    #[test]
    fn shutdown_rejects_normal_operation_admission() {
        struct InvalidCleanup;
        impl ManagedTask for InvalidCleanup {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start => TaskAction::RequestExit(3),
                    TaskEvent::ShutdownRequested { .. } => TaskAction::Pending {
                        label: "not cleanup safe".into(),
                        cleanup_safe: false,
                    },
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        runtime.start_main(Box::new(InvalidCleanup)).unwrap();
        let RunStatus::ShutdownComplete(report) = runtime.run_until_stalled().unwrap() else {
            panic!("shutdown must settle an initiator that violates admission rules");
        };
        assert_eq!(report.exit_code, 3);
        assert_eq!(
            report.initiator_outcome,
            TaskOutcome::Failed("ShutdownAdmissionClosed: operation".into())
        );
        assert!(runtime.take_operation_tickets().is_empty());
        assert!(runtime.resources().used.is_baseline());
    }

    #[test]
    fn first_exit_request_is_latched() {
        struct RepeatedExit;
        impl ManagedTask for RepeatedExit {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start => TaskAction::RequestExit(4),
                    TaskEvent::ShutdownRequested { .. } => TaskAction::RequestExit(99),
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        runtime.start_main(Box::new(RepeatedExit)).unwrap();
        let RunStatus::ShutdownComplete(report) = runtime.run_until_stalled().unwrap() else {
            panic!("repeated exit must still complete the first shutdown");
        };
        assert_eq!(report.exit_code, 4);
        assert_eq!(report.later_exit_requests, 1);
        assert_eq!(report.initiator_outcome, TaskOutcome::Cancelled);
        assert!(runtime.resources().used.is_baseline());
    }

    #[test]
    fn shutdown_control_progresses_while_user_slots_are_saturated() {
        struct SaturatedExit {
            remaining: usize,
        }
        impl ManagedTask for SaturatedExit {
            fn resume(&mut self, event: TaskEvent) -> TaskAction {
                match event {
                    TaskEvent::Start | TaskEvent::ChildSpawned(_) if self.remaining > 0 => {
                        self.remaining -= 1;
                        TaskAction::Spawn(Box::new(PendingOnce {
                            thread_ids: Arc::new(Mutex::new(Vec::new())),
                        }))
                    }
                    TaskEvent::ChildSpawned(_) => TaskAction::RequestExit(11),
                    TaskEvent::ShutdownRequested { exit_code: 11 } => TaskAction::Return,
                    other => TaskAction::Fail(format!("unexpected event: {other:?}")),
                }
            }
        }

        let resources = ResourceLimits {
            live_tasks: 5,
            pending_operations: 4,
            registrations: 0,
            queue_slots: 9,
            control_slots: 1,
        };
        let mut runtime = StandaloneRuntime::new(StandaloneOptions {
            resources,
            turn_limit: 1,
            cancellation_batch: 1,
        });
        runtime
            .start_main(Box::new(SaturatedExit { remaining: 4 }))
            .unwrap();
        let RunStatus::ShutdownComplete(report) = runtime.run_until_stalled().unwrap() else {
            panic!("reserved shutdown and cancellation control must make progress");
        };
        assert_eq!(report.exit_code, 11);
        let resources = runtime.resources();
        assert_eq!(resources.peak.queue_slots, resources.limits.queue_slots);
        assert_eq!(resources.peak.control_slots, 1);
        assert!(resources.used.is_baseline());
    }
}
