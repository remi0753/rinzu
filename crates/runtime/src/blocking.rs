//! Bounded pool for binding-declared offloadable blocking native calls.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

use crate::scheduler::{OperationId, ResolveResult, TaskId};
use crate::standalone::OperationTicket;

const DEFAULT_BLOCKING_THREADS: usize = 4;
const DEFAULT_BLOCKING_QUEUE: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOperationClass {
    BoundedSync,
    CompletionAsync,
    OffloadBlocking,
    HostLoop,
    RawUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeThreadRequirement {
    Any,
    ProcessInitial,
    HostOwner,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockingBindingMetadata {
    pub symbol: String,
    pub operation_class: NativeOperationClass,
    pub required_thread: NativeThreadRequirement,
    pub has_cancel_hook: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockingPoolLimits {
    pub threads: usize,
    pub queue_capacity: usize,
}

impl Default for BlockingPoolLimits {
    fn default() -> Self {
        Self {
            threads: DEFAULT_BLOCKING_THREADS,
            queue_capacity: DEFAULT_BLOCKING_QUEUE,
        }
    }
}

#[derive(Clone)]
pub struct CancelHook(Arc<dyn Fn() + Send + Sync>);

impl CancelHook {
    pub fn new(hook: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(hook))
    }

    fn invoke(&self) {
        (self.0)();
    }
}

impl fmt::Debug for CancelHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CancelHook(<native hook>)")
    }
}

#[derive(Clone, Debug)]
pub struct BlockingCancellationToken {
    requested: Arc<AtomicBool>,
}

impl BlockingCancellationToken {
    #[must_use]
    pub fn is_cancel_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockingOutput {
    pub bytes: Vec<u8>,
    pub thread: ThreadId,
}

type SharedBlockingResult = Arc<Mutex<Option<Result<BlockingOutput, String>>>>;

#[derive(Clone, Debug)]
pub struct BlockingOperation {
    state: SharedBlockingResult,
    requested: Arc<AtomicBool>,
    hook_invoked: Arc<AtomicBool>,
    hook: Option<CancelHook>,
    ticket: OperationTicket,
}

impl BlockingOperation {
    pub fn request_cancel(&self) -> ResolveResult {
        self.request_terminal(false)
    }

    pub fn request_timeout(&self) -> ResolveResult {
        self.request_terminal(true)
    }

    fn request_terminal(&self, timed_out: bool) -> ResolveResult {
        self.requested.store(true, Ordering::Release);
        if !self.hook_invoked.swap(true, Ordering::AcqRel)
            && let Some(hook) = &self.hook
        {
            hook.invoke();
        }
        if timed_out {
            self.ticket.timeout()
        } else {
            self.ticket.cancel()
        }
    }

    #[must_use]
    pub fn take(&self) -> Option<Result<BlockingOutput, String>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockingCompletion {
    pub operation: OperationId,
    pub task: TaskId,
    pub resolution: ResolveResult,
    pub thread: ThreadId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockingSubmitError {
    NotOffloadable {
        class: NativeOperationClass,
        required_thread: NativeThreadRequirement,
    },
    CancelMetadataMismatch,
    Saturated,
    Closed,
}

impl fmt::Display for BlockingSubmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOffloadable {
                class,
                required_thread,
            } => write!(
                formatter,
                "native call is not eligible for Blocking Pool: {class:?}, {required_thread:?}"
            ),
            Self::CancelMetadataMismatch => {
                formatter.write_str("binding cancel hook does not match metadata")
            }
            Self::Saturated => formatter.write_str("Blocking Pool queue is saturated"),
            Self::Closed => formatter.write_str("Blocking Pool is closed"),
        }
    }
}

impl std::error::Error for BlockingSubmitError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockingWaitError {
    Timeout,
    Closed,
}

impl fmt::Display for BlockingWaitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("Blocking Pool completion timed out"),
            Self::Closed => formatter.write_str("Blocking Pool completion channel is closed"),
        }
    }
}

impl std::error::Error for BlockingWaitError {}

type BlockingJob = Box<dyn FnOnce(BlockingCancellationToken) -> Result<Vec<u8>, String> + Send>;

struct BlockingItem {
    ticket: OperationTicket,
    job: BlockingJob,
    state: SharedBlockingResult,
    requested: Arc<AtomicBool>,
}

enum BlockingCommand {
    Run(BlockingItem),
    Stop,
}

pub struct BlockingPool {
    commands: Option<SyncSender<BlockingCommand>>,
    completions: Receiver<BlockingCompletion>,
    workers: Vec<JoinHandle<()>>,
}

impl BlockingPool {
    #[must_use]
    pub fn new() -> Self {
        Self::from_target_profile(BlockingPoolLimits::default())
    }

    #[must_use]
    pub fn from_target_profile(limits: BlockingPoolLimits) -> Self {
        assert!(
            limits.threads > 0,
            "Blocking Pool needs at least one thread"
        );
        assert!(
            limits.queue_capacity > 0,
            "Blocking Pool queue must be bounded and non-zero"
        );
        let (command_sender, command_receiver) = mpsc::sync_channel(limits.queue_capacity);
        let command_receiver = Arc::new(Mutex::new(command_receiver));
        let (completion_sender, completion_receiver) = mpsc::channel();
        let mut workers = Vec::with_capacity(limits.threads);
        for index in 0..limits.threads {
            let receiver = Arc::clone(&command_receiver);
            let completions = completion_sender.clone();
            workers.push(
                crate::allocation::checked_io(
                    thread::Builder::new()
                        .name(format!("rinzu-blocking-{index}"))
                        .spawn(move || blocking_loop(&receiver, &completions)),
                )
                .expect("create Blocking Pool thread"),
            );
        }
        Self {
            commands: Some(command_sender),
            completions: completion_receiver,
            workers,
        }
    }

    pub fn submit<F>(
        &self,
        ticket: OperationTicket,
        metadata: &BlockingBindingMetadata,
        cancel_hook: Option<CancelHook>,
        job: F,
    ) -> Result<BlockingOperation, BlockingSubmitError>
    where
        F: FnOnce(BlockingCancellationToken) -> Result<Vec<u8>, String> + Send + 'static,
    {
        if metadata.operation_class != NativeOperationClass::OffloadBlocking
            || metadata.required_thread != NativeThreadRequirement::Any
        {
            let error = BlockingSubmitError::NotOffloadable {
                class: metadata.operation_class,
                required_thread: metadata.required_thread,
            };
            let _ = ticket.complete(Err(error.to_string()));
            return Err(error);
        }
        if metadata.has_cancel_hook != cancel_hook.is_some() {
            let error = BlockingSubmitError::CancelMetadataMismatch;
            let _ = ticket.complete(Err(error.to_string()));
            return Err(error);
        }
        let state = Arc::new(Mutex::new(None));
        let requested = Arc::new(AtomicBool::new(false));
        let item = BlockingItem {
            ticket: ticket.clone(),
            job: Box::new(job),
            state: Arc::clone(&state),
            requested: Arc::clone(&requested),
        };
        let Some(commands) = &self.commands else {
            settle_rejected(item, "Blocking Pool is closed");
            return Err(BlockingSubmitError::Closed);
        };
        match commands.try_send(BlockingCommand::Run(item)) {
            Ok(()) => Ok(BlockingOperation {
                state,
                requested,
                hook_invoked: Arc::new(AtomicBool::new(false)),
                hook: cancel_hook,
                ticket,
            }),
            Err(TrySendError::Full(BlockingCommand::Run(item))) => {
                settle_rejected(item, "Blocking Pool queue is saturated");
                Err(BlockingSubmitError::Saturated)
            }
            Err(TrySendError::Disconnected(BlockingCommand::Run(item))) => {
                settle_rejected(item, "Blocking Pool is closed");
                Err(BlockingSubmitError::Closed)
            }
            Err(
                TrySendError::Full(BlockingCommand::Stop)
                | TrySendError::Disconnected(BlockingCommand::Stop),
            ) => {
                unreachable!("submit only sends blocking work")
            }
        }
    }

    pub fn wait_completion(
        &self,
        timeout: Duration,
    ) -> Result<BlockingCompletion, BlockingWaitError> {
        self.completions
            .recv_timeout(timeout)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => BlockingWaitError::Timeout,
                RecvTimeoutError::Disconnected => BlockingWaitError::Closed,
            })
    }
}

impl Default for BlockingPool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BlockingPool {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.take() {
            for _ in &self.workers {
                let _ = commands.send(BlockingCommand::Stop);
            }
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn blocking_loop(
    commands: &Mutex<Receiver<BlockingCommand>>,
    completions: &mpsc::Sender<BlockingCompletion>,
) {
    loop {
        let command = commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv();
        let Ok(command) = command else {
            return;
        };
        let BlockingCommand::Run(item) = command else {
            return;
        };
        let token = BlockingCancellationToken {
            requested: Arc::clone(&item.requested),
        };
        let result = catch_unwind(AssertUnwindSafe(|| (item.job)(token)))
            .unwrap_or_else(|_| Err("blocking native operation panicked".into()));
        let settlement = result.as_ref().map(|_| ()).map_err(Clone::clone);
        let thread = thread::current().id();
        *item
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(result.map(|bytes| BlockingOutput { bytes, thread }));
        let resolution = item.ticket.complete(settlement);
        if completions
            .send(BlockingCompletion {
                operation: item.ticket.operation,
                task: item.ticket.task,
                resolution,
                thread,
            })
            .is_err()
        {
            return;
        }
    }
}

fn settle_rejected(item: BlockingItem, message: &str) {
    *item
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Err(message.into()));
    let _ = item.ticket.complete(Err(message.into()));
}
