//! Main Domain scheduling primitives.
//!
//! These types deliberately contain no OS-thread scheduler. They model the
//! single-owner queue, structured task tree, and operation settlement rules
//! shared by the standalone pump and the future Host Loop adapter.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScopeId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OperationId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkItem {
    Task(TaskId),
    Operation(OperationId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ControlKey {
    CancelScope(ScopeId),
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlItem {
    pub key: ControlKey,
    pub cursor: Option<TaskId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueItem {
    Control(ControlItem),
    Work(WorkItem),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueResult {
    Queued { request_wake: bool },
    Coalesced,
}

#[derive(Debug, Eq, PartialEq)]
pub struct QueueTurn {
    pub items: Vec<QueueItem>,
    pub request_follow_up_wake: bool,
}

#[derive(Debug)]
pub struct WakeGate {
    armed: AtomicBool,
}

impl WakeGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
        }
    }

    /// Returns true exactly once until the owner acknowledges the wake.
    pub fn request(&self) -> bool {
        !self.armed.swap(true, Ordering::AcqRel)
    }

    pub fn acknowledge(&self) {
        self.armed.store(false, Ordering::Release);
    }
}

impl Default for WakeGate {
    fn default() -> Self {
        Self::new()
    }
}

/// A two-lane owner queue. Control work is always selected before user work,
/// while both lanes remain bounded by the number of items processed per turn.
#[derive(Debug)]
pub struct ManagedQueue {
    control: VecDeque<ControlItem>,
    work: VecDeque<WorkItem>,
    queued_control: HashSet<ControlKey>,
    queued_work: HashSet<WorkItem>,
    turn_limit: usize,
    wake_gate: Arc<WakeGate>,
}

impl ManagedQueue {
    #[must_use]
    pub fn new(turn_limit: usize, wake_gate: Arc<WakeGate>) -> Self {
        assert!(
            turn_limit > 0,
            "a queue turn must process at least one item"
        );
        Self {
            control: VecDeque::new(),
            work: VecDeque::new(),
            queued_control: HashSet::new(),
            queued_work: HashSet::new(),
            turn_limit,
            wake_gate,
        }
    }

    pub fn enqueue_work(&mut self, item: WorkItem) -> EnqueueResult {
        if !self.queued_work.insert(item) {
            return EnqueueResult::Coalesced;
        }
        self.work.push_back(item);
        EnqueueResult::Queued {
            request_wake: self.wake_gate.request(),
        }
    }

    pub fn enqueue_control(&mut self, item: ControlItem) -> EnqueueResult {
        if !self.queued_control.insert(item.key) {
            return EnqueueResult::Coalesced;
        }
        self.control.push_back(item);
        EnqueueResult::Queued {
            request_wake: self.wake_gate.request(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.control.is_empty() && self.work.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.control.len() + self.work.len()
    }

    #[must_use]
    pub const fn turn_limit(&self) -> usize {
        self.turn_limit
    }

    pub fn begin_turn(&self) {
        self.wake_gate.acknowledge();
    }

    pub fn pop_next(&mut self) -> Option<QueueItem> {
        if let Some(item) = self.control.pop_front() {
            self.queued_control.remove(&item.key);
            return Some(QueueItem::Control(item));
        }
        let item = self.work.pop_front()?;
        self.queued_work.remove(&item);
        Some(QueueItem::Work(item))
    }

    pub fn finish_turn(&self) -> bool {
        !self.is_empty() && self.wake_gate.request()
    }

    pub fn take_turn(&mut self) -> QueueTurn {
        self.begin_turn();
        let mut items = Vec::with_capacity(self.turn_limit.min(self.len()));
        while items.len() < self.turn_limit {
            let Some(item) = self.pop_next() else {
                break;
            };
            items.push(item);
        }

        let request_follow_up_wake = self.finish_turn();
        QueueTurn {
            items,
            request_follow_up_wake,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Returned,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopePhase {
    Open,
    ClosingNormal,
    ClosingFailure,
    ClosingCancelled,
    Closed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskState {
    Runnable,
    Running,
    Pending(OperationId),
    Closing(ScopePhase),
    Settled(TaskOutcome),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskTableError {
    UnknownTask(TaskId),
    UnknownScope(ScopeId),
    ScopeClosed(ScopeId),
    InvalidTransition {
        task: TaskId,
        from: TaskState,
        to: TaskState,
    },
}

impl fmt::Display for TaskTableError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTask(task) => write!(formatter, "unknown task {task:?}"),
            Self::UnknownScope(scope) => write!(formatter, "unknown scope {scope:?}"),
            Self::ScopeClosed(scope) => write!(formatter, "scope {scope:?} is closed"),
            Self::InvalidTransition { task, from, to } => {
                write!(
                    formatter,
                    "invalid transition for {task:?}: {from:?} -> {to:?}"
                )
            }
        }
    }
}

impl std::error::Error for TaskTableError {}

#[derive(Debug)]
struct TaskRecord {
    state: TaskState,
    parent_scope: ScopeId,
    child_scope: ScopeId,
    previous_sibling: Option<TaskId>,
    next_sibling: Option<TaskId>,
    cancel_requested: bool,
}

#[derive(Debug)]
struct ScopeRecord {
    owner: Option<TaskId>,
    phase: ScopePhase,
    first_child: Option<TaskId>,
    last_child: Option<TaskId>,
    child_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildBatch {
    pub children: Vec<TaskId>,
    pub next_cursor: Option<TaskId>,
}

/// Owns the structured task/scope tree. Child links live in task records, so a
/// cancellation walk does not allocate one queue item per child.
#[derive(Debug)]
pub struct TaskTable {
    tasks: HashMap<TaskId, TaskRecord>,
    scopes: HashMap<ScopeId, ScopeRecord>,
    root_scope: ScopeId,
    next_task: u64,
    next_scope: u64,
}

impl TaskTable {
    #[must_use]
    pub fn new() -> Self {
        let root_scope = ScopeId(0);
        let mut scopes = HashMap::new();
        scopes.insert(
            root_scope,
            ScopeRecord {
                owner: None,
                phase: ScopePhase::Open,
                first_child: None,
                last_child: None,
                child_count: 0,
            },
        );
        Self {
            tasks: HashMap::new(),
            scopes,
            root_scope,
            next_task: 0,
            next_scope: 1,
        }
    }

    #[must_use]
    pub fn root_scope(&self) -> ScopeId {
        self.root_scope
    }

    pub fn insert_task(&mut self, parent_scope: ScopeId) -> Result<TaskId, TaskTableError> {
        let parent = self
            .scopes
            .get(&parent_scope)
            .ok_or(TaskTableError::UnknownScope(parent_scope))?;
        if parent.phase != ScopePhase::Open {
            return Err(TaskTableError::ScopeClosed(parent_scope));
        }

        let task = TaskId(self.next_task);
        self.next_task += 1;
        let child_scope = ScopeId(self.next_scope);
        self.next_scope += 1;
        let previous_sibling = parent.last_child;

        self.scopes.insert(
            child_scope,
            ScopeRecord {
                owner: Some(task),
                phase: ScopePhase::Open,
                first_child: None,
                last_child: None,
                child_count: 0,
            },
        );
        self.tasks.insert(
            task,
            TaskRecord {
                state: TaskState::Runnable,
                parent_scope,
                child_scope,
                previous_sibling,
                next_sibling: None,
                cancel_requested: false,
            },
        );

        if let Some(previous) = previous_sibling {
            self.tasks
                .get_mut(&previous)
                .expect("scope child link must identify a live task")
                .next_sibling = Some(task);
        }
        let parent = self
            .scopes
            .get_mut(&parent_scope)
            .expect("parent scope was checked above");
        parent.first_child.get_or_insert(task);
        parent.last_child = Some(task);
        parent.child_count += 1;
        Ok(task)
    }

    pub fn set_running(&mut self, task: TaskId) -> Result<(), TaskTableError> {
        self.transition(task, TaskState::Running, |state| {
            matches!(state, TaskState::Runnable)
        })
    }

    pub fn set_pending(
        &mut self,
        task: TaskId,
        operation: OperationId,
    ) -> Result<(), TaskTableError> {
        self.transition(task, TaskState::Pending(operation), |state| {
            matches!(state, TaskState::Running)
        })
    }

    pub fn set_runnable(&mut self, task: TaskId) -> Result<(), TaskTableError> {
        self.transition(task, TaskState::Runnable, |state| {
            matches!(state, TaskState::Pending(_))
        })
    }

    pub fn yield_running(&mut self, task: TaskId) -> Result<(), TaskTableError> {
        self.transition(task, TaskState::Runnable, |state| {
            matches!(state, TaskState::Running)
        })
    }

    pub fn begin_close(&mut self, task: TaskId, phase: ScopePhase) -> Result<(), TaskTableError> {
        if !matches!(
            phase,
            ScopePhase::ClosingNormal | ScopePhase::ClosingFailure | ScopePhase::ClosingCancelled
        ) {
            let from = self.state(task)?.clone();
            return Err(TaskTableError::InvalidTransition {
                task,
                from,
                to: TaskState::Closing(phase),
            });
        }
        let child_scope = self.child_scope(task)?;
        self.transition(task, TaskState::Closing(phase), |state| {
            matches!(state, TaskState::Running)
        })?;
        self.scopes
            .get_mut(&child_scope)
            .expect("task child scope must be live")
            .phase = phase;
        Ok(())
    }

    pub fn settle(
        &mut self,
        task: TaskId,
        outcome: TaskOutcome,
    ) -> Result<ScopeId, TaskTableError> {
        let record = self
            .tasks
            .get(&task)
            .ok_or(TaskTableError::UnknownTask(task))?;
        let child_scope = self
            .scopes
            .get(&record.child_scope)
            .expect("task child scope must be live");
        if child_scope.child_count != 0 {
            return Err(TaskTableError::InvalidTransition {
                task,
                from: record.state.clone(),
                to: TaskState::Settled(outcome),
            });
        }
        let parent_scope = record.parent_scope;
        let previous = record.previous_sibling;
        let next = record.next_sibling;

        if let Some(previous) = previous {
            self.tasks
                .get_mut(&previous)
                .expect("previous sibling must be live")
                .next_sibling = next;
        }
        if let Some(next) = next {
            self.tasks
                .get_mut(&next)
                .expect("next sibling must be live")
                .previous_sibling = previous;
        }
        let parent = self
            .scopes
            .get_mut(&parent_scope)
            .expect("task parent scope must be live");
        if parent.first_child == Some(task) {
            parent.first_child = next;
        }
        if parent.last_child == Some(task) {
            parent.last_child = previous;
        }
        parent.child_count -= 1;

        let record = self.tasks.get_mut(&task).expect("task was checked above");
        record.state = TaskState::Settled(outcome);
        self.scopes
            .get_mut(&record.child_scope)
            .expect("task child scope must be live")
            .phase = ScopePhase::Closed;
        Ok(parent_scope)
    }

    pub fn request_cancel(&mut self, task: TaskId) -> Result<bool, TaskTableError> {
        let record = self
            .tasks
            .get_mut(&task)
            .ok_or(TaskTableError::UnknownTask(task))?;
        let was_requested = record.cancel_requested;
        record.cancel_requested = true;
        Ok(!was_requested)
    }

    pub fn cancel_requested(&self, task: TaskId) -> Result<bool, TaskTableError> {
        Ok(self
            .tasks
            .get(&task)
            .ok_or(TaskTableError::UnknownTask(task))?
            .cancel_requested)
    }

    pub fn state(&self, task: TaskId) -> Result<&TaskState, TaskTableError> {
        Ok(&self
            .tasks
            .get(&task)
            .ok_or(TaskTableError::UnknownTask(task))?
            .state)
    }

    pub fn parent_scope(&self, task: TaskId) -> Result<ScopeId, TaskTableError> {
        Ok(self
            .tasks
            .get(&task)
            .ok_or(TaskTableError::UnknownTask(task))?
            .parent_scope)
    }

    pub fn child_scope(&self, task: TaskId) -> Result<ScopeId, TaskTableError> {
        Ok(self
            .tasks
            .get(&task)
            .ok_or(TaskTableError::UnknownTask(task))?
            .child_scope)
    }

    pub fn scope_phase(&self, scope: ScopeId) -> Result<ScopePhase, TaskTableError> {
        Ok(self
            .scopes
            .get(&scope)
            .ok_or(TaskTableError::UnknownScope(scope))?
            .phase)
    }

    pub fn scope_owner(&self, scope: ScopeId) -> Result<Option<TaskId>, TaskTableError> {
        Ok(self
            .scopes
            .get(&scope)
            .ok_or(TaskTableError::UnknownScope(scope))?
            .owner)
    }

    pub fn child_count(&self, scope: ScopeId) -> Result<usize, TaskTableError> {
        Ok(self
            .scopes
            .get(&scope)
            .ok_or(TaskTableError::UnknownScope(scope))?
            .child_count)
    }

    pub fn first_child(&self, scope: ScopeId) -> Result<Option<TaskId>, TaskTableError> {
        Ok(self
            .scopes
            .get(&scope)
            .ok_or(TaskTableError::UnknownScope(scope))?
            .first_child)
    }

    pub fn next_sibling(&self, task: TaskId) -> Result<Option<TaskId>, TaskTableError> {
        Ok(self
            .tasks
            .get(&task)
            .ok_or(TaskTableError::UnknownTask(task))?
            .next_sibling)
    }

    pub fn child_batch(
        &self,
        scope: ScopeId,
        cursor: Option<TaskId>,
        limit: usize,
    ) -> Result<ChildBatch, TaskTableError> {
        let scope = self
            .scopes
            .get(&scope)
            .ok_or(TaskTableError::UnknownScope(scope))?;
        let mut current = cursor.or(scope.first_child);
        let mut children = Vec::with_capacity(limit.min(scope.child_count));
        while children.len() < limit {
            let Some(task) = current else {
                break;
            };
            let record = self
                .tasks
                .get(&task)
                .ok_or(TaskTableError::UnknownTask(task))?;
            children.push(task);
            current = record.next_sibling;
        }
        Ok(ChildBatch {
            children,
            next_cursor: current,
        })
    }

    fn transition(
        &mut self,
        task: TaskId,
        to: TaskState,
        allowed: impl FnOnce(&TaskState) -> bool,
    ) -> Result<(), TaskTableError> {
        let record = self
            .tasks
            .get_mut(&task)
            .ok_or(TaskTableError::UnknownTask(task))?;
        if !allowed(&record.state) {
            return Err(TaskTableError::InvalidTransition {
                task,
                from: record.state.clone(),
                to,
            });
        }
        record.state = to;
        Ok(())
    }
}

impl Default for TaskTable {
    fn default() -> Self {
        Self::new()
    }
}

const RESOLVER_PENDING: u8 = 0;
const RESOLVER_COMPLETING: u8 = 1;
const RESOLVER_CANCELLING: u8 = 2;
const RESOLVER_TIMING_OUT: u8 = 3;
const RESOLVER_SETTLED: u8 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolverState {
    Pending,
    Completing,
    Cancelling,
    TimingOut,
    Settled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution {
    Completed(Result<(), String>),
    Cancelled,
    TimedOut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionEvent {
    pub operation: OperationId,
    pub resolution: Resolution,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolveResult {
    Won,
    Late,
    RuntimeGone,
}

pub struct OperationResolver {
    operation: OperationId,
    state: AtomicU8,
    sender: Sender<ResolutionEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl fmt::Debug for OperationResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationResolver")
            .field("operation", &self.operation)
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl OperationResolver {
    #[must_use]
    pub fn new(
        operation: OperationId,
        sender: Sender<ResolutionEvent>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            operation,
            state: AtomicU8::new(RESOLVER_PENDING),
            sender,
            wake,
        }
    }

    #[must_use]
    pub fn operation(&self) -> OperationId {
        self.operation
    }

    #[must_use]
    pub fn state(&self) -> ResolverState {
        match self.state.load(Ordering::Acquire) {
            RESOLVER_PENDING => ResolverState::Pending,
            RESOLVER_COMPLETING => ResolverState::Completing,
            RESOLVER_CANCELLING => ResolverState::Cancelling,
            RESOLVER_TIMING_OUT => ResolverState::TimingOut,
            RESOLVER_SETTLED => ResolverState::Settled,
            _ => unreachable!("resolver state is only written by this type"),
        }
    }

    pub fn complete(&self, result: Result<(), String>) -> ResolveResult {
        self.resolve(RESOLVER_COMPLETING, Resolution::Completed(result))
    }

    pub fn cancel(&self) -> ResolveResult {
        self.resolve(RESOLVER_CANCELLING, Resolution::Cancelled)
    }

    pub fn timeout(&self) -> ResolveResult {
        self.resolve(RESOLVER_TIMING_OUT, Resolution::TimedOut)
    }

    pub fn mark_settled(&self) -> bool {
        self.state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                matches!(
                    state,
                    RESOLVER_COMPLETING | RESOLVER_CANCELLING | RESOLVER_TIMING_OUT
                )
                .then_some(RESOLVER_SETTLED)
            })
            .is_ok()
    }

    fn resolve(&self, winner: u8, resolution: Resolution) -> ResolveResult {
        if self
            .state
            .compare_exchange(
                RESOLVER_PENDING,
                winner,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return ResolveResult::Late;
        }
        if self
            .sender
            .send(ResolutionEvent {
                operation: self.operation,
                resolution,
            })
            .is_err()
        {
            self.state.store(RESOLVER_SETTLED, Ordering::Release);
            return ResolveResult::RuntimeGone;
        }
        (self.wake)();
        ResolveResult::Won
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    #[test]
    fn managed_queue_coalesces_and_bounds_turns() {
        let gate = Arc::new(WakeGate::new());
        let mut queue = ManagedQueue::new(2, gate);
        assert_eq!(
            queue.enqueue_work(WorkItem::Task(TaskId(1))),
            EnqueueResult::Queued { request_wake: true }
        );
        assert_eq!(
            queue.enqueue_work(WorkItem::Task(TaskId(1))),
            EnqueueResult::Coalesced
        );
        assert_eq!(
            queue.enqueue_work(WorkItem::Task(TaskId(2))),
            EnqueueResult::Queued {
                request_wake: false
            }
        );
        assert_eq!(
            queue.enqueue_control(ControlItem {
                key: ControlKey::Shutdown,
                cursor: None,
            }),
            EnqueueResult::Queued {
                request_wake: false
            }
        );

        let first = queue.take_turn();
        assert_eq!(first.items.len(), 2);
        assert!(matches!(first.items[0], QueueItem::Control(_)));
        assert_eq!(first.items[1], QueueItem::Work(WorkItem::Task(TaskId(1))));
        assert!(first.request_follow_up_wake);

        let second = queue.take_turn();
        assert_eq!(
            second.items,
            vec![QueueItem::Work(WorkItem::Task(TaskId(2)))]
        );
        assert!(!second.request_follow_up_wake);
    }

    #[test]
    fn task_table_keeps_an_intrusive_scope_child_list() {
        let mut table = TaskTable::new();
        let parent = table.insert_task(table.root_scope()).unwrap();
        let child_scope = table.child_scope(parent).unwrap();
        table.set_running(parent).unwrap();
        let children = (0..5)
            .map(|_| table.insert_task(child_scope).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(table.child_count(child_scope), Ok(5));

        let first = table.child_batch(child_scope, None, 2).unwrap();
        assert_eq!(first.children, children[0..2]);
        let second = table
            .child_batch(child_scope, first.next_cursor, 8)
            .unwrap();
        assert_eq!(second.children, children[2..5]);
        assert_eq!(second.next_cursor, None);

        table
            .begin_close(parent, ScopePhase::ClosingNormal)
            .unwrap();
        assert_eq!(
            table.insert_task(child_scope),
            Err(TaskTableError::ScopeClosed(child_scope))
        );
    }

    #[test]
    fn resolver_has_exactly_one_winner() {
        let (sender, receiver) = mpsc::channel();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_counter = Arc::clone(&wakes);
        let resolver = Arc::new(OperationResolver::new(
            OperationId(7),
            sender,
            Arc::new(move || {
                wake_counter.fetch_add(1, Ordering::Relaxed);
            }),
        ));
        let barrier = Arc::new(Barrier::new(4));
        let completion = {
            let resolver = Arc::clone(&resolver);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                resolver.complete(Ok(()))
            })
        };
        let cancellation = {
            let resolver = Arc::clone(&resolver);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                resolver.cancel()
            })
        };
        let timeout = {
            let resolver = Arc::clone(&resolver);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                resolver.timeout()
            })
        };
        barrier.wait();
        let results = [
            completion.join().unwrap(),
            cancellation.join().unwrap(),
            timeout.join().unwrap(),
        ];
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == ResolveResult::Won)
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == ResolveResult::Late)
                .count(),
            2
        );
        assert_eq!(receiver.try_iter().count(), 1);
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert!(resolver.mark_settled());
        assert_eq!(resolver.state(), ResolverState::Settled);
    }
}
