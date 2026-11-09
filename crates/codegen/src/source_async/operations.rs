//! Admission and completion of native, reactor and Worker operations.
use super::*;
use rinzu_runtime::failure_transport::{failure_descriptor, failure_value, materialize_failure};
use rinzu_runtime::reactor::{
    AsyncReactor, FileReadResult, ReactorCompletion, ReactorError, ReactorOperation,
};
use rinzu_runtime::scheduler::OperationId;
use rinzu_runtime::standalone::{OperationMailbox, StandaloneError};
use rinzu_runtime::worker::{CpuWorkerPool, WorkerCompletion, WorkerResult, WorkerWaitError};

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

pub(super) struct SourceOperations {
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
    pub(super) fn new(
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

    pub(super) fn start(&mut self, future: u64) -> Result<()> {
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

    pub(super) fn tracks(&self, future: u64) -> bool {
        self.active.contains_key(&future) || self.deferred_native.contains(&future)
    }

    pub(super) fn cancelled_children(
        &self,
        requested: &std::collections::BTreeSet<u64>,
    ) -> Vec<u64> {
        self.cancelled_children
            .borrow()
            .union(requested)
            .copied()
            .collect()
    }

    pub(super) fn request_control(&self, future: u64, timed: bool) -> bool {
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
    pub(super) fn cancel(&self, future: u64) -> bool {
        if self.active.contains_key(&future) || self.deferred_native.contains(&future) {
            let _ = self.request_control(future, false);
            true
        } else {
            false
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) fn has_ready_completion(&self) -> bool {
        self.completion_signal.epoch() != self.received_epoch
            || !self.reactor_completions.is_empty()
            || !self.worker_completions.is_empty()
    }

    pub(super) fn receive_one_ready(&mut self) -> Result<bool> {
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

    pub(super) fn receive_for_progress(&mut self, wanted: u64) -> Result<()> {
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

    pub(super) fn is_baseline(&self) -> bool {
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
    pub(super) fn finish_helpers(&mut self) {
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

#[cfg(test)]
#[path = "child_close_tests.rs"]
mod child_close_tests;

#[cfg(test)]
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
