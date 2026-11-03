//! Minimal one-shot timer and file-I/O reactor for the standalone runtime.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use crate::scheduler::{OperationId, ResolveResult, TaskId};
use crate::standalone::OperationTicket;

type SharedFileResult = Arc<Mutex<Option<Result<Vec<u8>, String>>>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReactorOperationKind {
    Timer,
    FileRead,
    NativeCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReactorTracePhase {
    Submitted,
    Started,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReactorTraceEvent {
    pub operation: OperationId,
    pub task: TaskId,
    pub kind: ReactorOperationKind,
    pub phase: ReactorTracePhase,
    pub thread: ThreadId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReactorCompletion {
    pub operation: OperationId,
    pub task: TaskId,
    pub kind: ReactorOperationKind,
    pub resolution: ResolveResult,
}

#[derive(Clone, Debug)]
pub struct FileReadResult {
    state: SharedFileResult,
    ticket: OperationTicket,
}

impl FileReadResult {
    #[must_use]
    pub fn take(&self) -> Option<Result<Vec<u8>, String>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub fn request_cancel(&self) -> ResolveResult {
        self.ticket.cancel()
    }

    pub fn request_timeout(&self) -> ResolveResult {
        self.ticket.timeout()
    }
}

#[derive(Clone, Debug)]
pub struct ReactorOperation {
    ticket: OperationTicket,
    native_completion: Option<crate::native_completion::CompletionPointer>,
    native_transfer: Option<Arc<AtomicI64>>,
}

impl ReactorOperation {
    /// Available after receiving this operation's physical completion.
    pub fn native_transfer_result(&self) -> Option<i64> {
        self.native_transfer
            .as_ref()
            .map(|value| value.load(Ordering::Acquire))
    }
    pub fn request_cancel(&self) -> ResolveResult {
        let resolution = self.ticket.cancel();
        if resolution == ResolveResult::Won
            && let Some(completion) = self.native_completion
        {
            completion.cancel();
        }
        resolution
    }

    pub fn request_timeout(&self) -> ResolveResult {
        let resolution = self.ticket.timeout();
        if resolution == ResolveResult::Won
            && let Some(completion) = self.native_completion
        {
            completion.cancel();
        }
        resolution
    }
}

#[derive(Debug)]
pub enum ReactorError {
    Closed,
    CompletionTimeout,
}

impl fmt::Display for ReactorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("async reactor is closed"),
            Self::CompletionTimeout => formatter.write_str("async reactor completion timed out"),
        }
    }
}

impl std::error::Error for ReactorError {}

enum ReactorCommand {
    Progress,
    Shutdown,
    NativeCompletion {
        ticket: OperationTicket,
        completion: crate::native_completion::CompletionPointer,
        transfer: Option<Arc<AtomicI64>>,
    },
    Timer {
        ticket: OperationTicket,
        duration: Duration,
    },
    FileRead {
        ticket: OperationTicket,
        path: PathBuf,
        result: SharedFileResult,
    },
}

struct PendingTimer {
    ticket: OperationTicket,
    started: Instant,
    duration: Duration,
}

impl PendingTimer {
    fn remaining(&self) -> Duration {
        self.duration.saturating_sub(self.started.elapsed())
    }
}

impl ReactorCommand {
    fn ticket(&self) -> &OperationTicket {
        match self {
            Self::Progress | Self::Shutdown => unreachable!("control commands have no ticket"),
            Self::Timer { ticket, .. }
            | Self::FileRead { ticket, .. }
            | Self::NativeCompletion { ticket, .. } => ticket,
        }
    }

    fn kind(&self) -> ReactorOperationKind {
        match self {
            Self::Progress | Self::Shutdown => unreachable!("control commands have no operation"),
            Self::Timer { .. } => ReactorOperationKind::Timer,
            Self::FileRead { .. } => ReactorOperationKind::FileRead,
            Self::NativeCompletion { .. } => ReactorOperationKind::NativeCompletion,
        }
    }
}

pub struct AsyncReactor {
    commands: Option<Sender<ReactorCommand>>,
    completions: Receiver<ReactorCompletion>,
    trace: Arc<Mutex<Vec<ReactorTraceEvent>>>,
    worker: Option<JoinHandle<()>>,
}

impl AsyncReactor {
    #[must_use]
    pub fn new() -> Self {
        Self::with_completion_wake(Arc::new(|| {}))
    }

    /// Wake after physical result/mailbox publication, using only thread-safe
    /// native metadata. The caller retains the target until reactor shutdown.
    #[must_use]
    pub fn with_completion_wake(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (command_sender, command_receiver) = mpsc::channel::<ReactorCommand>();
        let notification_sender = command_sender.clone();
        let native_wake = crate::native_completion::NativeCompletionWake::new(move || {
            // A retained producer can notify after reactor teardown. Sending to
            // the closed native channel then safely fails without heap access.
            let _ = notification_sender.send(ReactorCommand::Progress);
        });
        let (completion_sender, completion_receiver) = mpsc::channel();
        let trace = Arc::new(Mutex::new(Vec::new()));
        let worker_trace = Arc::clone(&trace);
        let worker = crate::allocation::checked_io(
            thread::Builder::new()
                .name("rinzu-io-reactor".into())
                .spawn(move || {
                    let mut timers: Vec<PendingTimer> = Vec::new();
                    let mut native: Vec<(
                        OperationTicket,
                        crate::native_completion::CompletionPointer,
                        Option<Arc<AtomicI64>>,
                    )> = Vec::new();
                    let mut closed = false;
                    loop {
                        let mut index = 0;
                        while index < native.len() {
                            if native[index].1.is_done() {
                                let (ticket, completion, transfer) = native.remove(index);
                                if let Some(value) = transfer {
                                    value.store(completion.transfer_result(), Ordering::Release);
                                }
                                if !complete(
                                    &completion_sender,
                                    &worker_trace,
                                    wake.as_ref(),
                                    &ticket,
                                    ReactorOperationKind::NativeCompletion,
                                    Ok(()),
                                ) {
                                    return;
                                }
                            } else {
                                index += 1;
                            }
                        }
                        // A timer retains its native ticket until its actual
                        // deadline. Sleeping for one timer must not serialize
                        // later timers or prevent file-service submission.
                        let mut index = 0;
                        while index < timers.len() {
                            if timers[index].remaining().is_zero() {
                                let timer = timers.remove(index);
                                if !complete(
                                    &completion_sender,
                                    &worker_trace,
                                    wake.as_ref(),
                                    &timer.ticket,
                                    ReactorOperationKind::Timer,
                                    Ok(()),
                                ) {
                                    return;
                                }
                            } else {
                                index += 1;
                            }
                        }
                        if closed && native.is_empty() && timers.is_empty() {
                            break;
                        }
                        let remaining = timers
                            .iter()
                            .map(PendingTimer::remaining)
                            .chain(
                                native
                                    .iter()
                                    .any(|(_, c, _)| !c.supports_notification())
                                    .then_some(Duration::from_millis(1)),
                            )
                            .min();
                        let command = if let Some(remaining) = remaining {
                            match command_receiver.recv_timeout(remaining) {
                                Ok(command) => command,
                                Err(RecvTimeoutError::Timeout) => continue,
                                Err(RecvTimeoutError::Disconnected) => {
                                    closed = true;
                                    continue;
                                }
                            }
                        } else {
                            match command_receiver.recv() {
                                Ok(command) => command,
                                Err(_) => {
                                    closed = true;
                                    continue;
                                }
                            }
                        };
                        match command {
                            ReactorCommand::Progress => continue,
                            ReactorCommand::Shutdown => {
                                closed = true;
                                continue;
                            }
                            _ => {}
                        }
                        let ticket = command.ticket().clone();
                        let kind = command.kind();
                        push_trace(&worker_trace, &ticket, kind, ReactorTracePhase::Started);
                        let result = match command {
                            ReactorCommand::NativeCompletion {
                                completion,
                                transfer,
                                ..
                            } => {
                                if completion.supports_notification() {
                                    completion.register_wake(&native_wake);
                                }
                                native.push((ticket, completion, transfer));
                                continue;
                            }
                            ReactorCommand::Timer { duration, .. } => {
                                timers.push(PendingTimer {
                                    ticket,
                                    started: Instant::now(),
                                    duration,
                                });
                                continue;
                            }
                            ReactorCommand::FileRead { path, result, .. } => {
                                let read = crate::allocation::checked_io(std::fs::read(&path))
                                    .map_err(|error| format!("read {}: {error}", path.display()));
                                let completion = read.as_ref().map(|_| ()).map_err(Clone::clone);
                                *result
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(read);
                                completion
                            }
                            ReactorCommand::Progress | ReactorCommand::Shutdown => unreachable!(),
                        };
                        if !complete(
                            &completion_sender,
                            &worker_trace,
                            wake.as_ref(),
                            &ticket,
                            kind,
                            result,
                        ) {
                            break;
                        }
                    }
                }),
        )
        .expect("create async reactor thread");
        Self {
            commands: Some(command_sender),
            completions: completion_receiver,
            trace,
            worker: Some(worker),
        }
    }

    pub fn submit_timer(
        &self,
        ticket: OperationTicket,
        duration: Duration,
    ) -> Result<ReactorOperation, ReactorError> {
        push_trace(
            &self.trace,
            &ticket,
            ReactorOperationKind::Timer,
            ReactorTracePhase::Submitted,
        );
        self.commands
            .as_ref()
            .ok_or(ReactorError::Closed)?
            .send(ReactorCommand::Timer {
                ticket: ticket.clone(),
                duration,
            })
            .map_err(|_| ReactorError::Closed)?;
        Ok(ReactorOperation {
            ticket,
            native_completion: None,
            native_transfer: None,
        })
    }

    pub fn submit_native_completion(
        &self,
        ticket: OperationTicket,
        completion: crate::native_completion::CompletionPointer,
    ) -> Result<ReactorOperation, ReactorError> {
        self.submit_native_receipt(ticket, completion, None)
    }
    pub fn submit_native_transfer(
        &self,
        ticket: OperationTicket,
        completion: crate::native_completion::CompletionPointer,
    ) -> Result<ReactorOperation, ReactorError> {
        completion.require_transfer();
        self.submit_native_receipt(ticket, completion, Some(Arc::new(AtomicI64::new(0))))
    }
    fn submit_native_receipt(
        &self,
        ticket: OperationTicket,
        completion: crate::native_completion::CompletionPointer,
        transfer: Option<Arc<AtomicI64>>,
    ) -> Result<ReactorOperation, ReactorError> {
        push_trace(
            &self.trace,
            &ticket,
            ReactorOperationKind::NativeCompletion,
            ReactorTracePhase::Submitted,
        );
        self.commands
            .as_ref()
            .ok_or(ReactorError::Closed)?
            .send(ReactorCommand::NativeCompletion {
                ticket: ticket.clone(),
                completion,
                transfer: transfer.clone(),
            })
            .map_err(|_| ReactorError::Closed)?;
        Ok(ReactorOperation {
            ticket,
            native_completion: Some(completion),
            native_transfer: transfer,
        })
    }

    pub fn submit_file_read(
        &self,
        ticket: OperationTicket,
        path: impl Into<PathBuf>,
    ) -> Result<FileReadResult, ReactorError> {
        push_trace(
            &self.trace,
            &ticket,
            ReactorOperationKind::FileRead,
            ReactorTracePhase::Submitted,
        );
        let state = Arc::new(Mutex::new(None));
        self.commands
            .as_ref()
            .ok_or(ReactorError::Closed)?
            .send(ReactorCommand::FileRead {
                ticket: ticket.clone(),
                path: path.into(),
                result: Arc::clone(&state),
            })
            .map_err(|_| ReactorError::Closed)?;
        Ok(FileReadResult { state, ticket })
    }

    pub fn wait_completion(&self, timeout: Duration) -> Result<ReactorCompletion, ReactorError> {
        self.completions
            .recv_timeout(timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => ReactorError::CompletionTimeout,
                RecvTimeoutError::Disconnected => ReactorError::Closed,
            })
    }

    pub fn try_completion(&self) -> Result<Option<ReactorCompletion>, ReactorError> {
        match self.completions.try_recv() {
            Ok(completion) => Ok(Some(completion)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ReactorError::Closed),
        }
    }

    #[must_use]
    pub fn trace(&self) -> Vec<ReactorTraceEvent> {
        self.trace
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn complete(
    sender: &Sender<ReactorCompletion>,
    trace: &Mutex<Vec<ReactorTraceEvent>>,
    wake: &dyn Fn(),
    ticket: &OperationTicket,
    kind: ReactorOperationKind,
    result: Result<(), String>,
) -> bool {
    let resolution = ticket.complete(result);
    push_trace(trace, ticket, kind, ReactorTracePhase::Completed);
    if sender
        .send(ReactorCompletion {
            operation: ticket.operation,
            task: ticket.task,
            kind,
            resolution,
        })
        .is_err()
    {
        return false;
    }
    ticket.publish_readiness();
    wake();
    true
}

impl Default for AsyncReactor {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncReactor {
    pub fn shutdown(&mut self) {
        if let Some(sender) = self.commands.take() {
            // Notification targets retain a channel sender beyond teardown, so
            // shutdown is explicit rather than relying on sender disconnection.
            let _ = sender.send(ReactorCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for AsyncReactor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn push_trace(
    trace: &Mutex<Vec<ReactorTraceEvent>>,
    ticket: &OperationTicket,
    kind: ReactorOperationKind,
    phase: ReactorTracePhase,
) {
    trace
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(ReactorTraceEvent {
            operation: ticket.operation,
            task: ticket.task,
            kind,
            phase,
            thread: thread::current().id(),
        });
}
