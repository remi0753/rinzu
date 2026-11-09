//! Per-frame failure routing shared by typed source continuations and Workers.
use std::collections::{HashMap, VecDeque};

use anyhow::{Result, anyhow, ensure};
use rinzu_compiler::{
    BasicBlock, BlockId, IrFunction, IrModule, Operation, OperationKind, SourceRange,
};
use rinzu_runtime::failure::{Diagnostic, Failure, FailureCause, PendingFailure, SourcePosition};

#[derive(Clone, Debug)]
pub(super) struct ManagedFailure(pub Failure);
impl std::fmt::Display for ManagedFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0.primary.cause.message())
    }
}
impl std::error::Error for ManagedFailure {}

type Route = (VecDeque<BlockId>, Option<BlockId>);
pub(super) struct ScopeReleasePlan {
    pub scopes: Vec<SourceRange>,
    pub ended_cleanups: Vec<SourceRange>,
    pub after: Option<SourceRange>,
}
pub(super) struct FailureRoutes {
    groups: rinzu_runtime::error_groups::ErrorGroups,
    pending: PendingFailure,
    sources: HashMap<u32, u64>,
    cleanup_scopes: HashMap<SourceRange, BlockId>,
    cleanup_aliases: HashMap<BlockId, SourceRange>,
    catch_in_cleanup: Vec<BlockId>,
    active: Vec<SourceRange>,
    route: Route,
    saved: HashMap<BlockId, Route>,
    failure: BlockId,
    ended_cleanups: Vec<SourceRange>,
    scope_release: Option<ScopeReleasePlan>,
    scope_finally: HashMap<SourceRange, BlockId>,
}
impl FailureRoutes {
    pub fn set_tracer(&mut self, tracer: rinzu_runtime::error_trace::TraceContext) {
        self.pending.set_tracer(tracer);
    }
    pub fn tracer(&self) -> &rinzu_runtime::error_trace::TraceContext {
        self.pending.tracer()
    }
    pub fn new(module: &IrModule, function: &IrFunction) -> Self {
        let cleanup_scopes: HashMap<_, _> = function
            .blocks
            .iter()
            .filter(|block| {
                block
                    .operations
                    .iter()
                    .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
            })
            .map(|block| (block.range, block.id))
            .collect();
        let catch_in_cleanup = function
            .blocks
            .iter()
            .filter(|block| {
                block.operations.iter().any(|op| {
                    matches!(
                        op.kind,
                        OperationKind::CatchError { .. }
                            | OperationKind::CatchErrors { .. }
                            | OperationKind::RecordTaskError { .. }
                    )
                }) && block
                    .entry_scopes
                    .iter()
                    .any(|scope| cleanup_scopes.contains_key(scope))
            })
            .map(|block| block.id)
            .collect();
        Self {
            groups: Default::default(),
            pending: PendingFailure::default(),
            sources: crate::baseline::source_identities(module).0,
            cleanup_aliases: function
                .blocks
                .iter()
                .filter_map(|b| {
                    b.operations.iter().find_map(|op| {
                        if let OperationKind::TaskScopeOutcome { scope, .. }
                        | OperationKind::ErrorGroupOutcome { scope, .. } = op.kind
                        {
                            Some((b.id, scope))
                        } else {
                            None
                        }
                    })
                })
                .collect(),
            cleanup_scopes,
            catch_in_cleanup,
            active: Vec::new(),
            route: (VecDeque::new(), None),
            saved: HashMap::new(),
            failure: function.failure,
            ended_cleanups: Vec::new(),
            scope_release: None,
            scope_finally: function
                .scopes
                .iter()
                .filter_map(|scope| scope.deferred_finally.last().map(|id| (scope.scope, *id)))
                .collect(),
        }
    }
    pub fn diagnostic(&self, cause: FailureCause, range: SourceRange) -> Diagnostic {
        Diagnostic {
            cause,
            source: Some(SourcePosition {
                file: self
                    .sources
                    .get(&range.file_id.0)
                    .copied()
                    .unwrap_or(u64::from(range.file_id.0)),
                start_byte: range.start_byte,
                end_byte: range.end_byte,
            }),
        }
    }
    pub fn enter(&mut self, block: &BasicBlock) {
        self.active = block
            .entry_scopes
            .iter()
            .filter(|scope| self.cleanup_scopes.contains_key(scope))
            .copied()
            .collect();
        if self.cleanup_scopes.values().any(|id| *id == block.id) {
            self.pending.begin_cleanup_scoped(0, u64::from(block.id.0));
        }
    }
    pub fn before(&mut self, operation: &Operation) {
        self.pending.tracer().source(
            self.diagnostic(FailureCause::RuntimePanic, operation.range)
                .source,
        );
        match operation.kind {
            OperationKind::EnterScope { scope } if self.cleanup_scopes.contains_key(&scope) => {
                self.active.push(scope)
            }
            OperationKind::ExitScope { scope, .. } if self.cleanup_scopes.contains_key(&scope) => {
                self.active.retain(|active| *active != scope)
            }
            _ => {}
        }
    }
    pub fn cleanup_incoming(&self) -> Option<Failure> {
        self.pending.cleanup_incoming().cloned()
    }
    pub fn enter_error_group(&mut self, scope: SourceRange) {
        self.groups
            .enter((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte));
    }
    pub fn record_error(
        &mut self,
        scope: SourceRange,
        origin: Option<u64>,
        block: BlockId,
    ) -> Result<()> {
        let failure = self.catch(block)?;
        self.groups
            .record(
                (u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte),
                origin,
                failure,
            )
            .map_err(|error| anyhow!("invalid lexical Error record: {error:?}"))?;
        Ok(())
    }
    pub fn finish_error_group(
        &mut self,
        scope: SourceRange,
        handles: bool,
    ) -> Result<Vec<Failure>> {
        let failures = self
            .groups
            .finish((u64::from(scope.start_byte) << 32) | u64::from(scope.end_byte))
            .map_err(|error| anyhow!("invalid lexical Error finish: {error:?}"))?;
        let incoming = self
            .pending
            .replace_cleanup_incoming(None)
            .map_err(|error| anyhow!("invalid Error cleanup: {error:?}"))?;
        let (handled, incoming) = rinzu_runtime::error_groups::settle(failures, incoming, handles);
        self.pending
            .replace_cleanup_incoming(incoming)
            .map_err(|error| anyhow!("invalid Error cleanup: {error:?}"))?;
        Ok(handled)
    }
    pub fn settle_task_join(&mut self, scope: SourceRange, failure: Option<Failure>) -> Result<()> {
        let region = self.cleanup_scopes[&scope];
        self.pending
            .settle_task_join(0, u64::from(region.0), failure)
            .map_err(|error| anyhow!("invalid task group cleanup: {error:?}"))
    }
    pub fn in_cleanup(&self) -> bool {
        self.pending.cleanup_depth() != 0
    }
    fn end_cleanup(&mut self, scope: SourceRange) -> Result<()> {
        let region = self.cleanup_scopes[&scope];
        let ended = self
            .pending
            .end_cleanup_scoped(0, u64::from(region.0))
            .map_err(|error| anyhow!("invalid source cleanup state: {error:?}"))?;
        if ended {
            self.ended_cleanups.push(scope);
        }
        Ok(())
    }
    pub fn finish_task_scope(&mut self, cleanup: BlockId) -> Result<Option<Failure>> {
        let scope = self.cleanup_range(cleanup);
        self.end_cleanup(scope)?;
        self.active.retain(|active| *active != scope);
        Ok(self.pending.current().cloned())
    }
    pub fn cleanup_range(&self, block: BlockId) -> SourceRange {
        self.cleanup_scopes
            .iter()
            .find_map(|(scope, id)| (*id == block).then_some(*scope))
            .or_else(|| self.cleanup_aliases.get(&block).copied())
            .expect("verified cleanup block")
    }
    pub fn scope_finally(&self, scope: SourceRange) -> Option<SourceRange> {
        self.scope_finally
            .get(&scope)
            .map(|id| self.cleanup_range(*id))
    }
    fn release_scopes(&mut self, scopes: &[SourceRange], cleanup: &[BlockId]) {
        self.scope_release = Some(ScopeReleasePlan {
            scopes: scopes.to_vec(),
            ended_cleanups: std::mem::take(&mut self.ended_cleanups),
            after: cleanup.last().map(|id| self.cleanup_range(*id)),
        });
    }
    pub fn take_scope_release(&mut self) -> Option<ScopeReleasePlan> {
        self.scope_release.take()
    }
    fn jump(&mut self, cleanup: &[BlockId], target: BlockId) -> BlockId {
        let mut path: VecDeque<_> = cleanup.iter().copied().chain([target]).collect();
        let first = path.pop_front().unwrap();
        self.route = (path, Some(first));
        first
    }
    pub fn propagate(&mut self, operation: &Operation, failure: Failure) -> Result<BlockId> {
        self.pending.propagate(failure);
        let (error, cleanup) = match &operation.kind {
            OperationKind::Field {
                error: Some(error), ..
            }
            | OperationKind::FormatString {
                error: Some(error), ..
            } => (
                Some(error),
                operation
                    .unwind
                    .as_ref()
                    .map_or(&[][..], |plan| plan.finally.as_slice()),
            ),
            OperationKind::ThrowError {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::RethrowCaught {
                error,
                cleanup_chain,
                ..
            } => (Some(error), cleanup_chain.as_slice()),
            OperationKind::CheckLocal {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::TaskScopeOutcome {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::ErrorGroupOutcome {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::Stdout {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::NumericConvert {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::ManagedCall {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::IndirectCall {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::Await {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::SettleSupplant {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::NativeCall {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::DataOperation {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::SourceNativeCall {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::ReadNativeResult {
                error,
                cleanup_chain,
                ..
            }
            | OperationKind::PrepareNativeCall {
                error,
                cleanup_chain,
                ..
            } => (Some(error), cleanup_chain.as_slice()),
            OperationKind::Unary { cleanup_chain, .. }
            | OperationKind::Binary { cleanup_chain, .. }
            | OperationKind::Spawn { cleanup_chain, .. }
            | OperationKind::CancellationPoll { cleanup_chain } => (None, cleanup_chain.as_slice()),
            _ => (
                None,
                operation
                    .unwind
                    .as_ref()
                    .map_or(&[][..], |plan| plan.finally.as_slice()),
            ),
        };
        if let Some(error) =
            error.filter(|_| self.pending.current().is_some_and(Failure::recoverable))
        {
            if self.catch_in_cleanup.contains(&error.target) {
                self.saved.insert(error.target, self.route.clone());
            }
            let released = error
                .release_scopes
                .iter()
                .filter(|scope| self.cleanup_scopes.contains_key(scope))
                .copied()
                .collect::<Vec<_>>();
            for scope in released {
                self.end_cleanup(scope)?;
            }
            if self.pending.current().is_some_and(Failure::recoverable) {
                self.release_scopes(&error.release_scopes, &error.cleanup_chain);
                return Ok(self.jump(&error.cleanup_chain, error.target));
            }
            let remaining = self
                .active
                .iter()
                .rev()
                .filter(|scope| !error.release_scopes.contains(scope))
                .copied()
                .collect::<Vec<_>>();
            for scope in remaining {
                self.end_cleanup(scope)?;
            }
            self.release_scopes(
                operation
                    .unwind
                    .as_ref()
                    .map_or(&[], |plan| &plan.release_scopes),
                cleanup,
            );
            return Ok(self.jump(cleanup, self.failure));
        }
        for scope in self.active.clone().into_iter().rev() {
            self.end_cleanup(scope)?;
        }
        self.release_scopes(
            operation
                .unwind
                .as_ref()
                .map_or(&[], |plan| &plan.release_scopes),
            cleanup,
        );
        Ok(self.jump(cleanup, self.failure))
    }
    pub fn catch(&mut self, block: BlockId) -> Result<Failure> {
        if let Some(route) = self.saved.remove(&block) {
            self.route = route;
        }
        self.pending
            .observe_error()
            .ok_or_else(|| anyhow!("catch entered without a recoverable source Error"))
    }
    pub fn goto(&mut self, block: &BasicBlock, target: BlockId) -> Result<BlockId> {
        let cleanup_exit =
            block
                .operations
                .iter()
                .rev()
                .find_map(|operation| match operation.kind {
                    OperationKind::ExitScope { scope, .. } => {
                        self.cleanup_scopes.get(&scope).copied()
                    }
                    _ => None,
                });
        if let Some(cleanup) = cleanup_exit {
            let scope = self
                .cleanup_scopes
                .iter()
                .find_map(|(scope, region)| (*region == cleanup).then_some(*scope))
                .unwrap();
            self.end_cleanup(scope)?;
            if self.route.1 == Some(cleanup) || self.route.1 == Some(block.id) {
                let next = self
                    .route
                    .0
                    .pop_front()
                    .ok_or_else(|| anyhow!("source cleanup route has no destination"))?;
                self.route.1 = Some(next);
                self.release_scopes(&[], &[]);
                return Ok(next);
            }
            if self.pending.current().is_some() {
                self.release_scopes(&[], &[]);
                return Ok(self.failure);
            }
        }
        if let rinzu_compiler::Terminator::Goto {
            cleanup_chain,
            release_scopes,
            ..
        } = &block.terminator
        {
            self.release_scopes(release_scopes, cleanup_chain);
            if !cleanup_chain.is_empty() {
                return Ok(self.jump(cleanup_chain, target));
            }
        }
        Ok(target)
    }
    pub fn return_target(&mut self, block: &BasicBlock) -> Option<BlockId> {
        let rinzu_compiler::Terminator::Return {
            cleanup_chain,
            release_scopes,
            ..
        } = &block.terminator
        else {
            return None;
        };
        if cleanup_chain.is_empty() || self.route.1 == Some(block.id) {
            return None;
        }
        self.release_scopes(release_scopes, cleanup_chain);
        Some(self.jump(cleanup_chain, block.id))
    }
    pub fn retains_return(&self, block: BlockId) -> bool {
        let contains = |route: &Route| route.1 == Some(block) || route.0.contains(&block);
        contains(&self.route) || self.saved.values().any(contains)
    }
    pub fn finish(&mut self) -> Result<Option<Failure>> {
        ensure!(
            self.pending.cleanup_depth() == 0,
            "source frame finished with active cleanup"
        );
        std::mem::take(&mut self.pending)
            .finish()
            .map_err(|error| anyhow!("invalid source failure state: {error:?}"))
    }
}
