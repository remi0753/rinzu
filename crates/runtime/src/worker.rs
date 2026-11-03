//! Runtime-owned CPU worker pool and one-shot isolated Worker Domains.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

use crate::heap::DomainHeap;
use crate::scheduler::{OperationId, ResolveResult, TaskId};
use crate::standalone::OperationTicket;

const MAX_CPU_WORKERS: usize = 8;
const QUEUE_ITEMS_PER_WORKER: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkerDomainId(pub u64);

#[derive(Debug)]
pub struct WorkerDomain {
    id: WorkerDomainId,
    owner_thread: ThreadId,
    heap: DomainHeap,
    cancellation: Arc<AtomicBool>,
}

impl WorkerDomain {
    #[must_use]
    pub fn id(&self) -> WorkerDomainId {
        self.id
    }

    #[must_use]
    pub fn owner_thread(&self) -> ThreadId {
        self.owner_thread
    }

    #[must_use]
    pub fn is_cancel_requested(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    pub fn heap_mut(&mut self) -> &mut DomainHeap {
        &mut self.heap
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerEvidence {
    pub domain: WorkerDomainId,
    pub thread: ThreadId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerOutput {
    pub bytes: Vec<u8>,
    pub evidence: WorkerEvidence,
}

type SharedWorkerResult = Arc<Mutex<Option<Result<WorkerOutput, String>>>>;

#[derive(Clone, Debug)]
pub struct WorkerResult {
    state: SharedWorkerResult,
    cancellation: Arc<AtomicBool>,
    ticket: OperationTicket,
}

impl WorkerResult {
    pub fn request_cancel(&self) -> ResolveResult {
        self.cancellation.store(true, Ordering::Release);
        self.ticket.cancel()
    }

    pub fn request_timeout(&self) -> ResolveResult {
        self.cancellation.store(true, Ordering::Release);
        self.ticket.timeout()
    }

    #[must_use]
    pub fn take(&self) -> Option<Result<WorkerOutput, String>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerCompletion {
    pub operation: OperationId,
    pub task: TaskId,
    pub domain: WorkerDomainId,
    pub resolution: ResolveResult,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerSubmitError {
    Saturated,
    Closed,
}

impl fmt::Display for WorkerSubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Saturated => formatter.write_str("CPU worker queue is saturated"),
            Self::Closed => formatter.write_str("CPU worker pool is closed"),
        }
    }
}

impl std::error::Error for WorkerSubmitError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkerWaitError {
    Timeout,
    Closed,
}

impl fmt::Display for WorkerWaitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("CPU worker completion timed out"),
            Self::Closed => formatter.write_str("CPU worker completion channel is closed"),
        }
    }
}

impl std::error::Error for WorkerWaitError {}

type WorkerJob = Box<dyn FnOnce(&mut WorkerDomain) -> Result<Vec<u8>, String> + Send>;

struct WorkItem {
    ticket: OperationTicket,
    domain: WorkerDomainId,
    job: WorkerJob,
    state: SharedWorkerResult,
    cancellation: Arc<AtomicBool>,
}

enum WorkerCommand {
    Run(WorkItem),
    Stop,
}

pub struct CpuWorkerPool {
    commands: Option<SyncSender<WorkerCommand>>,
    completions: Receiver<WorkerCompletion>,
    workers: Vec<JoinHandle<()>>,
    next_domain: AtomicU64,
}

impl CpuWorkerPool {
    #[must_use]
    pub fn new() -> Self {
        Self::with_completion_wake(Arc::new(|| {}))
    }

    /// Publish native packets and end the Worker Domain before notifying the
    /// Owner. Wake never reads the Owner heap and stays pinned through shutdown.
    #[must_use]
    pub fn with_completion_wake(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let worker_count = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .saturating_sub(1)
            .clamp(1, MAX_CPU_WORKERS);
        let (command_sender, command_receiver) =
            mpsc::sync_channel(worker_count * QUEUE_ITEMS_PER_WORKER);
        let command_receiver = Arc::new(Mutex::new(command_receiver));
        let (completion_sender, completion_receiver) = mpsc::channel();
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let receiver = Arc::clone(&command_receiver);
            let completions = completion_sender.clone();
            let wake = Arc::clone(&wake);
            workers.push(
                crate::allocation::checked_io(
                    thread::Builder::new()
                        .name(format!("rinzu-cpu-worker-{index}"))
                        .spawn(move || worker_loop(&receiver, &completions, &*wake)),
                )
                .expect("create CPU worker thread"),
            );
        }
        Self {
            commands: Some(command_sender),
            completions: completion_receiver,
            workers,
            next_domain: AtomicU64::new(1),
        }
    }

    pub fn spawn<F>(
        &self,
        ticket: OperationTicket,
        job: F,
    ) -> Result<WorkerResult, WorkerSubmitError>
    where
        F: FnOnce(&mut WorkerDomain) -> Result<Vec<u8>, String> + Send + 'static,
    {
        let domain = WorkerDomainId(self.next_domain.fetch_add(1, Ordering::Relaxed));
        let state = Arc::new(Mutex::new(None));
        let cancellation = Arc::new(AtomicBool::new(false));
        let operation_ticket = ticket.clone();
        let item = WorkItem {
            ticket,
            domain,
            job: Box::new(job),
            state: Arc::clone(&state),
            cancellation: Arc::clone(&cancellation),
        };
        let Some(commands) = &self.commands else {
            settle_rejected(item, "CPU worker pool is closed");
            return Err(WorkerSubmitError::Closed);
        };
        match commands.try_send(WorkerCommand::Run(item)) {
            Ok(()) => Ok(WorkerResult {
                state,
                cancellation,
                ticket: operation_ticket,
            }),
            Err(TrySendError::Full(WorkerCommand::Run(item))) => {
                settle_rejected(item, "CPU worker queue is saturated");
                Err(WorkerSubmitError::Saturated)
            }
            Err(TrySendError::Disconnected(WorkerCommand::Run(item))) => {
                settle_rejected(item, "CPU worker pool is closed");
                Err(WorkerSubmitError::Closed)
            }
            Err(
                TrySendError::Full(WorkerCommand::Stop)
                | TrySendError::Disconnected(WorkerCommand::Stop),
            ) => {
                unreachable!("spawn only sends work")
            }
        }
    }

    pub fn wait_completion(&self, timeout: Duration) -> Result<WorkerCompletion, WorkerWaitError> {
        self.completions
            .recv_timeout(timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => WorkerWaitError::Timeout,
                RecvTimeoutError::Disconnected => WorkerWaitError::Closed,
            })
    }
    pub fn try_completion(&self) -> Result<Option<WorkerCompletion>, WorkerWaitError> {
        match self.completions.try_recv() {
            Ok(completion) => Ok(Some(completion)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerWaitError::Closed),
        }
    }
}

impl Default for CpuWorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

impl CpuWorkerPool {
    pub fn shutdown(&mut self) {
        if let Some(commands) = self.commands.take() {
            for _ in &self.workers {
                let _ = commands.send(WorkerCommand::Stop);
            }
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
impl Drop for CpuWorkerPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(
    commands: &Mutex<Receiver<WorkerCommand>>,
    completions: &mpsc::Sender<WorkerCompletion>,
    wake: &(dyn Fn() + Send + Sync),
) {
    loop {
        let command = commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv();
        let Ok(command) = command else {
            return;
        };
        let WorkerCommand::Run(item) = command else {
            return;
        };
        let mut domain = WorkerDomain {
            id: item.domain,
            owner_thread: thread::current().id(),
            heap: DomainHeap::new(item.domain.0),
            cancellation: Arc::clone(&item.cancellation),
        };
        let result = if domain.is_cancel_requested() {
            // An unstarted closure can own encoded inputs and admission. Drop
            // them on the Worker before exposing cancellation completion.
            drop(item.job);
            Err("worker operation cancelled before start".into())
        } else {
            catch_unwind(AssertUnwindSafe(|| (item.job)(&mut domain)))
                .unwrap_or_else(|_| Err("worker operation panicked".into()))
        };
        // Completion is published after the Domain heap has ended on its owner.
        drop(domain);
        let settlement = result.as_ref().map(|_| ()).map_err(Clone::clone);
        *item
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(result.map(|bytes| WorkerOutput {
                bytes,
                evidence: WorkerEvidence {
                    domain: item.domain,
                    thread: thread::current().id(),
                },
            }));
        let resolution = item.ticket.complete(settlement);
        if completions
            .send(WorkerCompletion {
                operation: item.ticket.operation,
                task: item.ticket.task,
                domain: item.domain,
                resolution,
            })
            .is_err()
        {
            return;
        }
        item.ticket.publish_readiness();
        wake();
    }
}

fn settle_rejected(item: WorkItem, message: &str) {
    drop(item.job);
    *item
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Err(message.into()));
    let _ = item.ticket.complete(Err(message.into()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spawn::{OriginDomainId, SpawnAdmissionController};
    use crate::standalone::{
        ManagedTask, RunStatus, StandaloneOptions, StandaloneRuntime, TaskAction, TaskEvent,
    };
    use crate::transport::{TransferDescriptor, TransferKind, TransferValue};
    use std::time::Duration;

    #[test]
    fn native_safe_blocks_keep_every_cpu_slot_while_io_and_owner_progress() {
        use crate::reactor::AsyncReactor;
        use crate::resources::ResourceLimits;
        use crate::standalone::OperationMailbox;
        use crate::transition::TransitionMachine;
        let pool = CpuWorkerPool::new();
        let count = pool.workers.len();
        let mut mailbox = OperationMailbox::new(ResourceLimits::default(), Arc::new(|| {}));
        let (started, starts) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Arc::new(Mutex::new(released));
        let mut results = Vec::new();
        for index in 0..count {
            let started = started.clone();
            let released = Arc::clone(&released);
            results.push(
                pool.spawn(
                    mailbox
                        .admit(TaskId(index as u64 + 1), "blocked native".into())
                        .unwrap(),
                    move |domain| {
                        let machine = Arc::new(TransitionMachine::new());
                        machine.activate_managed().unwrap();
                        let scope = machine
                            .native_scope(crate::transition::RootPublication {
                                stack_map_id: 1,
                                root_count: 0,
                            })
                            .unwrap();
                        let owner = thread::current().id();
                        assert_eq!(domain.owner_thread(), owner);
                        started.send(owner).unwrap();
                        released
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap();
                        assert_eq!(thread::current().id(), owner);
                        drop(scope);
                        machine.deactivate_managed().unwrap();
                        Ok(vec![1])
                    },
                )
                .unwrap(),
            );
        }
        let mut threads = std::collections::HashSet::new();
        for _ in 0..count {
            assert!(threads.insert(starts.recv_timeout(Duration::from_secs(5)).unwrap()));
        }
        let (queued, queued_start) = mpsc::channel();
        let queued_result = pool
            .spawn(
                mailbox.admit(TaskId(100), "queued CPU".into()).unwrap(),
                move |_| {
                    queued.send(thread::current().id()).unwrap();
                    Ok(vec![2])
                },
            )
            .unwrap();
        let reactor = AsyncReactor::new();
        let path =
            std::env::temp_dir().join(format!("rinzu-cpu-blocked-io-{}", std::process::id()));
        std::fs::write(&path, b"I/O progresses with every CPU slot blocked").unwrap();
        let file = reactor
            .submit_file_read(
                mailbox
                    .admit(TaskId(101), "independent I/O".into())
                    .unwrap(),
                &path,
            )
            .unwrap();
        reactor.wait_completion(Duration::from_secs(5)).unwrap();
        assert_eq!(
            file.take().unwrap().unwrap(),
            b"I/O progresses with every CPU slot blocked"
        );
        assert!(matches!(
            queued_start.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        assert!(
            results
                .iter()
                .all(|result| result.state.lock().unwrap().is_none())
        );
        assert_eq!(pool.workers.len(), count);
        // Release even if a subsequent assertion fails; Drop never invents a native return.
        for _ in 0..count {
            release.send(()).unwrap();
        }
        let queued_thread = queued_start.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(threads.contains(&queued_thread));
        for _ in 0..=count {
            pool.wait_completion(Duration::from_secs(5)).unwrap();
        }
        for result in results {
            assert_eq!(result.take().unwrap().unwrap().bytes, [1]);
        }
        assert_eq!(queued_result.take().unwrap().unwrap().bytes, [2]);
        for event in mailbox.take_resolutions().unwrap() {
            mailbox.settle(event.operation).unwrap();
        }
        assert!(mailbox.resources().used.is_baseline());
        std::fs::remove_file(path).unwrap();
    }

    struct Pending;
    impl ManagedTask for Pending {
        fn resume(&mut self, event: TaskEvent) -> TaskAction {
            match event {
                TaskEvent::Start => TaskAction::Pending {
                    label: "cancel-before-start".into(),
                    cleanup_safe: false,
                },
                TaskEvent::Operation(crate::scheduler::Resolution::Cancelled) => {
                    TaskAction::Cancelled
                }
                event => TaskAction::Fail(format!("unexpected {event:?}")),
            }
        }
    }

    #[test]
    fn unstarted_job_releases_transport_and_admission_before_completion_is_visible() {
        let owner = thread::current().id();
        let mut runtime = StandaloneRuntime::new(StandaloneOptions::default());
        runtime.start_main(Box::new(Pending)).unwrap();
        assert!(matches!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Stalled { .. }
        ));
        let ticket = runtime.take_operation_tickets().pop().unwrap();
        ticket.cancel();
        let controller = SpawnAdmissionController::new();
        let origin = OriginDomainId(9);
        let mut admitted = controller.admit(origin, 8).unwrap();
        let buffer = admitted
            .encode_input(
                &TransferDescriptor {
                    kind: TransferKind::Int,
                    source: None,
                },
                &TransferValue::Int(7),
            )
            .unwrap();
        let dropped_on = Arc::new(Mutex::new(None));
        struct Probe(Arc<Mutex<Option<ThreadId>>>);
        impl Drop for Probe {
            fn drop(&mut self) {
                *self.0.lock().unwrap() = Some(thread::current().id());
            }
        }
        let probe = Probe(Arc::clone(&dropped_on));
        let state = Arc::new(Mutex::new(None));
        let item = WorkItem {
            ticket,
            domain: WorkerDomainId(1),
            state: Arc::clone(&state),
            cancellation: Arc::new(AtomicBool::new(true)),
            job: Box::new(move |_| {
                drop((probe, buffer, admitted));
                panic!("cancelled job must never enter");
            }),
        };
        let (commands, receiver) = mpsc::sync_channel(1);
        let (sender, completions) = mpsc::channel();
        let worker = thread::spawn(move || worker_loop(&Mutex::new(receiver), &sender, &|| {}));
        commands.send(WorkerCommand::Run(item)).unwrap();
        completions.recv_timeout(Duration::from_secs(2)).unwrap();
        // Read immediately at publication, without joining or waiting for a later turn.
        assert_ne!(dropped_on.lock().unwrap().unwrap(), owner);
        let snapshot = controller.snapshot(origin);
        assert_eq!(snapshot.worker_domains, 0);
        assert_eq!(snapshot.in_flight_for_origin, 0);
        assert_eq!(snapshot.transport_bytes, 0);
        assert!(state.lock().unwrap().as_ref().unwrap().is_err());
        commands.send(WorkerCommand::Stop).unwrap();
        worker.join().unwrap();
        assert!(matches!(
            runtime.run_until_stalled().unwrap(),
            RunStatus::Complete(crate::scheduler::TaskOutcome::Cancelled)
        ));
        assert!(runtime.resources().used.is_baseline());
    }
}
