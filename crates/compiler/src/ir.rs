pub(crate) mod collection_buffers;
mod dead_roots;
mod proven_owner;
pub use proven_owner::proven_data_owner_operations;
mod proven_roots;
pub use proven_roots::{proven_frame_memberships, proven_frame_roots};
mod fresh_futures;
pub use fresh_futures::fresh_return_consuming_awaits;
mod errors;
mod virtual_options;
pub use dead_roots::{dead_local_roots, dead_temporary_roots};
use std::collections::{HashMap, HashSet};
pub use virtual_options::virtual_bytes_options;

use crate::ast::{
    Arguments, BinaryOperator, Block, Expression, ExpressionKind, Literal, Statement,
    StatementKind, UnaryOperator,
};
use crate::sema::{CheckedFunction, CheckedModule, FunctionFacts, NativeCallClass, Type};
use crate::{Diagnostic, DiagnosticCode, SourceRange};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BlockId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ValueId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeDomain {
    Origin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AwaitReadyPath {
    ContinueInline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AwaitPendingPath {
    SuspendAndResume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnAdmission {
    Required,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTransition {
    NativeSafe,
    OwnerThreadSync,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReservationId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NativeCallId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallbackArgumentPlan {
    pub index: usize,
    pub targets: Vec<String>,
    /// Typed closure wrapper passed only to a private managed adapter.
    pub capture: Option<Type>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeEntryDomain {
    CurrentCaller,
    ProcessMain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitReason {
    Normal,
    Return,
    Break,
    Continue,
    Failure,
    Cancellation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PollReason {
    FunctionEntry,
    LoopBackedge,
    ManagedCall,
    NativeCall,
    Allocation,
    Await,
    Spawn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StackRoot {
    Value { value: ValueId, ty: Type },
    Local { name: String, ty: Type },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StackMap {
    pub roots: Vec<StackRoot>,
}

#[derive(Clone, Debug)]
pub struct IrModule {
    pub functions: Vec<IrFunction>,
    /// Canonical nominal identities with fields and variants in declaration order.
    pub source_types: std::collections::BTreeMap<String, crate::SourceTypeSchema>,
    /// Package-qualified logical paths, independent of process-local parse IDs.
    pub source_files: std::collections::BTreeMap<u32, String>,
    /// Only positions used by executable IR; no runtime source-file reads.
    pub source_locations:
        std::collections::BTreeMap<u32, std::collections::BTreeMap<u32, crate::SourceCoordinates>>,
    pub source_native_signatures: Vec<crate::SourceNativeSignature>,
    pub native_records: std::collections::BTreeMap<String, Vec<(String, crate::AbiScalar)>>,
}

#[derive(Clone, Debug)]
pub struct IrFunction {
    pub name: String,
    pub is_async: bool,
    pub result_type: Type,
    pub parameter_types: Vec<Type>,
    pub callback_fallback: Option<crate::FallbackConstant>,
    pub facts: FunctionFacts,
    pub scopes: Vec<ScopeInfo>,
    pub entry: BlockId,
    pub failure: BlockId,
    pub blocks: Vec<BasicBlock>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub id: BlockId,
    pub operations: Vec<Operation>,
    pub entry_scopes: Vec<SourceRange>,
    pub terminator: Terminator,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct ScopeInfo {
    pub scope: SourceRange,
    pub parent: Option<SourceRange>,
    pub locals: Vec<String>,
    pub deferred_finally: Vec<BlockId>,
}

/// Call roots release first; finally then runs before lexical roots and storage release.
#[derive(Clone, Debug)]
pub struct UnwindPlan {
    pub finally: Vec<BlockId>,
    pub release_scopes: Vec<SourceRange>,
    pub release_native_calls: Vec<NativeCallId>,
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub result: Option<TypedValue>,
    pub kind: OperationKind,
    pub unwind: Option<UnwindPlan>,
    pub range: SourceRange,
}

#[derive(Clone, Debug)]
pub struct TypedValue {
    pub id: ValueId,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstantValue {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
}

/// Only operation Error takes this edge; traps and cancellation use failure/cleanup_chain.
#[derive(Clone, Debug)]
pub struct ErrorEdge {
    pub target: BlockId,
    pub cleanup_chain: Vec<BlockId>,
    pub release_scopes: Vec<SourceRange>,
    pub release_native_calls: Vec<NativeCallId>,
}

#[derive(Clone, Debug)]
pub enum OperationKind {
    EnterErrorGroup {
        scope: SourceRange,
    },
    FinishErrorGroup {
        scope: SourceRange,
        handles: bool,
    },
    ErrorGroupOutcome {
        scope: SourceRange,
        cleanup: BlockId,
        error: ErrorEdge,
        failure: BlockId,
        cleanup_chain: Vec<BlockId>,
    },
    DeclareLocal {
        name: String,
    },
    CheckLocal {
        name: String,
        error: ErrorEdge,
        failure: BlockId,
        cleanup_chain: Vec<BlockId>,
    },
    RecordTaskError {
        scope: SourceRange,
    },
    TakeTaskErrors {
        scope: SourceRange,
    },
    TaskControl {
        kind: crate::TaskControlKind,
        future: Option<ValueId>,
    },
    FormatString {
        parts: Vec<(ValueId, Type)>,
        error: Option<ErrorEdge>,
        failure: Option<BlockId>,
        cleanup_chain: Vec<BlockId>,
    },
    Stdout {
        operand: ValueId,
        ty: Type,
        newline: bool,
        error: ErrorEdge,
        failure: BlockId,
        cleanup_chain: Vec<BlockId>,
    },
    ErrorMessage {
        error: ValueId,
    },
    ErrorCast {
        operand: ValueId,
    },
    ErrorIs {
        operand: ValueId,
        target: Type,
    },
    ErrorField {
        error: ValueId,
        field: u32,
    },
    ErrorMatch {
        errors: ValueId,
        types: Vec<Type>,
        rest: bool,
    },
    ErrorIndex {
        errors: ValueId,
        index: u32,
    },
    ErrorRest {
        errors: ValueId,
        start: u32,
    },
    CatchErrors {
        observes_failure: bool,
    },
    ThrowError {
        value: ValueId,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    CoreLength {
        operand: ValueId,
        kind: crate::CoreLengthKind,
    },
    DataOperation {
        kind: crate::DataOperation,
        arguments: Vec<ValueId>,
        argument_types: Vec<Type>,
        error: ErrorEdge,
        failure: BlockId,
        cleanup_chain: Vec<BlockId>,
    },
    Field {
        operand: ValueId,
        identity: String,
        field: u32,
        error: Option<ErrorEdge>,
        failure: Option<BlockId>,
        cleanup_chain: Vec<BlockId>,
    },
    /// Direct typed projection; the finite List loop proves the index bound.
    ListElement {
        operand: ValueId,
        index: ValueId,
    },
    EnumPayload {
        operand: ValueId,
        identity: String,
        variant: u32,
        field: u32,
    },
    EnterScope {
        scope: SourceRange,
    },
    ExitScope {
        scope: SourceRange,
        deferred_finally: Vec<BlockId>,
    },
    Parameter {
        index: u32,
    },
    AsyncStart,
    Constant(ConstantValue),
    LoadLocal {
        name: String,
    },
    FunctionReference {
        target: String,
    },
    IndirectCall {
        callee: ValueId,
        targets: Vec<String>,
        arguments: Vec<ValueId>,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    StaticHandlerReference {
        target: String,
    },
    StoreField {
        operand: ValueId,
        identity: String,
        field: u32,
        value: ValueId,
    },
    StoreLocal {
        name: String,
        value: ValueId,
    },
    Unary {
        operator: UnaryOperator,
        operand: ValueId,
        failure: Option<BlockId>,
        cleanup_chain: Vec<BlockId>,
    },
    BytesPad {
        operand: ValueId,
        from: u64,
        to: u64,
    },
    NumericConvert {
        operand: ValueId,
        source: Type,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    Binary {
        operator: BinaryOperator,
        left: ValueId,
        right: ValueId,
        failure: Option<BlockId>,
        cleanup_chain: Vec<BlockId>,
    },
    ManagedCall {
        target: String,
        arguments: Vec<ValueId>,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    CatchError {
        observes_failure: bool,
    },
    ReserveSupplant {
        reservation: ReservationId,
        callee: Option<ValueId>,
        targets: Vec<String>,
        arguments: Vec<ValueId>,
        retained: Vec<ValueId>,
        signature: crate::FunctionReferenceType,
        callbacks: Vec<CallbackArgumentPlan>,
    },
    StartSupplant {
        reservation: ReservationId,
        entry_domain: NativeEntryDomain,
        suspend_before_entry: bool,
    },
    SettleSupplant {
        reservation: ReservationId,
        release: bool,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    PrepareNativeCall {
        call: NativeCallId,
        signature: crate::SourceNativeSignature,
        arguments: Vec<ValueId>,
        retained: Vec<ValueId>,
        callbacks: Vec<CallbackArgumentPlan>,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    CloseNativeCall {
        call: NativeCallId,
    },
    SettleNativeAdmissions {
        call: NativeCallId,
        origin: ResumeDomain,
    },
    ReadNativeResult {
        call: NativeCallId,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    ReleaseNativeCall {
        call: NativeCallId,
    },
    SourceNativeCall {
        call: Option<NativeCallId>,
        signature: crate::SourceNativeSignature,
        arguments: Vec<ValueId>,
        failure: BlockId,
        error: ErrorEdge,
        cleanup_chain: Vec<BlockId>,
    },
    NativeCall {
        target: String,
        arguments: Vec<ValueId>,
        failure: BlockId,
        error: ErrorEdge,
        transition: NativeTransition,
        cleanup_chain: Vec<BlockId>,
    },
    Construct {
        target: String,
        /// Some only for a closed enum; arguments are in declaration order.
        variant: Option<u32>,
        arguments: Vec<ValueId>,
        failure: BlockId,
        cleanup_chain: Vec<BlockId>,
    },
    Await {
        future: ValueId,
        /// A freshly admitted, unaliased Future consumed by this direct await.
        consume: bool,
        failure: BlockId,
        error: ErrorEdge,
        ready: AwaitReadyPath,
        pending: AwaitPendingPath,
        resume_domain: ResumeDomain,
        cleanup_chain: Vec<BlockId>,
    },
    Spawn {
        target: String,
        arguments: Vec<ValueId>,
        failure: BlockId,
        admission: SpawnAdmission,
        settle_domain: ResumeDomain,
        cleanup_chain: Vec<BlockId>,
    },
    CancellationPoll {
        cleanup_chain: Vec<BlockId>,
    },
    GcSafepoint {
        reason: PollReason,
        stack_map: StackMap,
    },
    EnterTaskScope {
        all: bool,
        scope: SourceRange,
        body_scope: SourceRange,
        cleanup_scope: SourceRange,
    },
    JoinTaskScope {
        scope: SourceRange,
        cleanup: bool,
    },
    TaskScopeOutcome {
        scope: SourceRange,
        cleanup: BlockId,
        error: ErrorEdge,
        failure: BlockId,
        cleanup_chain: Vec<BlockId>,
    },
    EnterTry {
        cleanup: BlockId,
    },
    LeaveTry {
        cleanup: BlockId,
    },
}

#[derive(Clone, Debug)]
pub enum Terminator {
    Pending,
    Goto {
        target: BlockId,
        cleanup_chain: Vec<BlockId>,
        release_scopes: Vec<SourceRange>,
        reason: ExitReason,
    },
    Branch {
        condition: ValueId,
        then_target: BlockId,
        else_target: BlockId,
    },
    Switch {
        value: ValueId,
        identity: String,
        cases: Vec<(u32, BlockId)>,
    },
    Return {
        value: Option<ValueId>,
        cleanup_chain: Vec<BlockId>,
        release_scopes: Vec<SourceRange>,
    },
    Fail,
    Unreachable,
}

pub(crate) fn lower(checked: &CheckedModule) -> IrModule {
    let mut ir = IrModule {
        source_locations: Default::default(),
        source_files: checked
            .closure_source
            .map(|file| (file.0, "<managed closure conversion>".to_owned()))
            .into_iter()
            .collect(),
        source_types: checked.source_types.clone(),
        source_native_signatures: checked.source_native_signatures.clone(),
        native_records: checked
            .syntax
            .native_blocks
            .iter()
            .flat_map(|block| &block.members)
            .filter_map(|member| match member {
                crate::ast::NativeMember::Record(record) => Some((
                    record.name.clone(),
                    record
                        .fields
                        .iter()
                        .map(|field| {
                            let crate::ast::TypeRefKind::Named(path) = &field.ty.kind else {
                                unreachable!("scalar fields validated")
                            };
                            (
                                field.name.clone(),
                                crate::AbiScalar::from_name(&path[0]).expect("scalar field"),
                            )
                        })
                        .collect(),
                )),
                _ => None,
            })
            .collect(),
        functions: checked
            .functions
            .iter()
            .map(|function| Builder::new(checked, function).lower())
            .collect(),
    };
    let collections = crate::collections::source_file();
    if ir
        .functions
        .iter()
        .any(|function| function.range.file_id == collections.id())
    {
        ir.source_files
            .insert(collections.id().0, collections.path().display().to_string());
        ir.embed_source_locations([collections]);
    }
    ir
}

struct Builder<'a> {
    unbound_locals: HashSet<String>,
    control_epilogues: Vec<(BlockId, usize, usize)>,
    all_scope: Option<SourceRange>,
    all_dispatch: HashMap<SourceRange, (SourceRange, Vec<crate::ast::CatchClause>)>,
    all_outcomes: HashSet<SourceRange>,
    all_bodies: HashMap<SourceRange, SourceRange>,
    checked: &'a CheckedModule,
    function: &'a CheckedFunction,
    blocks: Vec<BasicBlock>,
    current: BlockId,
    failure: BlockId,
    next_value: u32,
    next_reservation: u32,
    next_native_call: u32,
    active_native_calls: Vec<NativeCallId>,
    loop_stack: Vec<(BlockId, BlockId, usize, usize)>,
    cleanup_stack: Vec<BlockId>,
    task_joins: HashMap<SourceRange, (SourceRange, bool)>,
    catch_stack: Vec<(BlockId, usize, usize)>,
    scope_stack: Vec<SourceRange>,
    scopes: Vec<ScopeInfo>,
    deferred_finally: HashMap<SourceRange, Vec<BlockId>>,
    local_types: HashMap<String, Type>,
    value_definitions: HashMap<ValueId, (BlockId, Type, Option<SourceRange>)>,
    expression_spills: HashMap<ValueId, String>,
    expression_reloads: HashMap<(BlockId, ValueId), ValueId>,
    buffered_updates: HashSet<SourceRange>,
}

impl<'a> Builder<'a> {
    fn new(checked: &'a CheckedModule, function: &'a CheckedFunction) -> Self {
        let range = function.declaration.range;
        let entry = BasicBlock {
            id: BlockId(0),
            operations: Vec::new(),
            entry_scopes: Vec::new(),
            terminator: Terminator::Pending,
            range,
        };
        let failure = BasicBlock {
            id: BlockId(1),
            operations: Vec::new(),
            entry_scopes: Vec::new(),
            terminator: Terminator::Fail,
            range,
        };
        Self {
            unbound_locals: HashSet::new(),
            control_epilogues: Vec::new(),
            all_scope: None,
            all_dispatch: HashMap::new(),
            all_outcomes: HashSet::new(),
            all_bodies: HashMap::new(),
            checked,
            function,
            blocks: vec![entry, failure],
            current: BlockId(0),
            failure: BlockId(1),
            next_value: 0,
            next_reservation: 0,
            next_native_call: 0,
            active_native_calls: Vec::new(),
            loop_stack: Vec::new(),
            cleanup_stack: Vec::new(),
            task_joins: HashMap::new(),
            catch_stack: Vec::new(),
            scope_stack: Vec::new(),
            scopes: Vec::new(),
            deferred_finally: HashMap::new(),
            local_types: HashMap::new(),
            value_definitions: HashMap::new(),
            expression_spills: HashMap::new(),
            expression_reloads: HashMap::new(),
            buffered_updates: HashSet::new(),
        }
    }

    fn lower(mut self) -> IrFunction {
        let frame_scope = self.function.declaration.range;
        self.scopes.push(ScopeInfo {
            scope: frame_scope,
            parent: None,
            locals: self
                .function
                .declaration
                .parameters
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            deferred_finally: Vec::new(),
        });
        self.scope_stack.push(frame_scope);
        self.emit(
            OperationKind::EnterScope { scope: frame_scope },
            None,
            frame_scope,
        );
        for (index, (parameter, ty)) in self
            .function
            .declaration
            .parameters
            .iter()
            .zip(&self.function.parameter_types)
            .enumerate()
        {
            let value = self.emit_value(
                OperationKind::Parameter {
                    index: index as u32,
                },
                ty.clone(),
                parameter.range,
            );
            self.emit(
                OperationKind::StoreLocal {
                    name: parameter.name.clone(),
                    value,
                },
                None,
                parameter.range,
            );
            self.local_types.insert(parameter.name.clone(), ty.clone());
        }
        self.emit_poll(PollReason::FunctionEntry, self.function.declaration.range);
        if self.function.declaration.is_async {
            self.emit(
                OperationKind::AsyncStart,
                None,
                self.function.declaration.range,
            );
        }
        self.lower_block(&self.function.declaration.body);
        if self.is_open(self.current) {
            if self.function.result_type == Type::Unit {
                self.set_terminator(Terminator::Return {
                    value: None,
                    cleanup_chain: self.cleanup_chain(),
                    release_scopes: self.scope_stack.iter().rev().copied().collect(),
                });
            } else {
                // Checked non-Unit bodies return on every normal path. A join
                // left open after those exits cannot return an implicit Unit.
                self.set_terminator(Terminator::Unreachable);
            }
        }
        IrFunction {
            name: self.function.declaration.name.clone(),
            is_async: self.function.declaration.is_async,
            result_type: self.function.result_type.clone(),
            parameter_types: self.function.parameter_types.clone(),
            callback_fallback: self.function.callback_fallback.clone(),
            facts: self.function.facts.clone(),
            scopes: self.scopes,
            entry: BlockId(0),
            failure: self.failure,
            blocks: self.blocks,
            range: self.function.declaration.range,
        }
    }

    fn lower_block(&mut self, block: &Block) {
        self.lower_block_with_prelude(block, |_| {});
    }

    fn lower_block_with_prelude(&mut self, block: &Block, prelude: impl FnOnce(&mut Self)) {
        let previous_all = self.all_scope;
        if let Some(scope) = self.all_bodies.get(&block.range) {
            self.all_scope = Some(*scope);
        }
        if self.task_joins.contains_key(&block.range) {
            self.all_scope = None;
        }

        let before: HashSet<_> = self.local_types.keys().cloned().collect();
        let deferred_finally = self
            .deferred_finally
            .get(&block.range)
            .cloned()
            .unwrap_or_default();
        self.scopes.push(ScopeInfo {
            scope: block.range,
            parent: self.scope_stack.last().copied(),
            locals: Vec::new(),
            deferred_finally: deferred_finally.clone(),
        });
        self.scope_stack.push(block.range);
        self.emit(
            OperationKind::EnterScope { scope: block.range },
            None,
            block.range,
        );
        if let Some(&(scope, cleanup)) = self.task_joins.get(&block.range) {
            self.emit_gc(PollReason::Await, block.range);
            self.emit(
                OperationKind::JoinTaskScope { scope, cleanup },
                None,
                block.range,
            );
        }
        let epilogue = if self.all_outcomes.contains(&block.range) {
            let range = SourceRange {
                instance: block.range.instance.wrapping_add(0x60000000),
                ..block.range
            };
            let epilogue = self.new_block(range);
            self.control_epilogues.push((
                epilogue,
                self.cleanup_stack.len(),
                self.scope_stack.len(),
            ));
            Some(epilogue)
        } else {
            None
        };
        prelude(self);
        if let Some((scope, handlers)) = self.all_dispatch.get(&block.range).cloned() {
            self.lower_all_dispatch(scope, &handlers, block.range);
        }
        let buffers = if self.all_scope.is_none() {
            collection_buffers::loops(self.checked, block)
        } else {
            HashMap::new()
        };
        for statement in &block.statements {
            if !self.is_open(self.current) {
                self.current = self.new_block(statement.range);
            }
            if let Some(buffers) = buffers.get(&statement.range) {
                for buffer in buffers {
                    self.buffered_updates.extend(&buffer.updates);
                }
                self.lower_continuing_statement(statement);
                for buffer in buffers {
                    self.finish_collection(&buffer.local, statement.range);
                    for update in &buffer.updates {
                        self.buffered_updates.remove(update);
                    }
                }
            } else {
                self.lower_continuing_statement(statement);
            }
        }
        if let Some(epilogue) = epilogue {
            self.control_epilogues.pop();
            self.goto_if_open(epilogue, ExitReason::Normal);
            self.current = epilogue;
        }
        if self.is_open(self.current) {
            if self.all_outcomes.contains(&block.range) {
                let cleanup = self
                    .blocks
                    .iter()
                    .find(|candidate| {
                        candidate.range == block.range
                            && candidate
                                .operations
                                .iter()
                                .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
                    })
                    .expect("all cleanup entry")
                    .id;
                self.emit(
                    OperationKind::ErrorGroupOutcome {
                        scope: block.range,
                        cleanup,
                        error: self.error_edge(),
                        failure: self.failure,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    None,
                    block.range,
                );
            }
            if self
                .task_joins
                .get(&block.range)
                .is_some_and(|(_, cleanup)| !cleanup)
            {
                let cleanup = self
                    .blocks
                    .iter()
                    .find(|b| {
                        b.range == block.range
                            && b.operations
                                .iter()
                                .any(|op| matches!(op.kind, OperationKind::LeaveTry { .. }))
                    })
                    .expect("task cleanup entry")
                    .id;
                self.emit(
                    OperationKind::TaskScopeOutcome {
                        scope: block.range,
                        cleanup,
                        error: self.error_edge(),
                        failure: self.failure,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    None,
                    block.range,
                );
            }
            self.emit(
                OperationKind::ExitScope {
                    scope: block.range,
                    deferred_finally,
                },
                None,
                block.range,
            );
        }
        self.scope_stack.pop();
        self.local_types.retain(|name, _| before.contains(name));
        self.all_scope = previous_all;
    }

    fn lower_continuing_statement(&mut self, statement: &Statement) {
        let Some(scope) = self.all_scope else {
            self.lower_statement(statement);
            return;
        };
        let caught_range = SourceRange {
            instance: statement
                .range
                .instance
                .wrapping_add(0x50000000 + self.blocks.len() as u32),
            ..statement.range
        };
        let caught = self.new_block(caught_range);
        let next = self.new_block(SourceRange {
            instance: caught_range.instance + 1,
            ..caught_range
        });
        self.catch_stack
            .push((caught, self.cleanup_stack.len(), self.scope_stack.len()));
        self.lower_statement(statement);
        self.catch_stack.pop();
        self.goto_if_open(next, ExitReason::Normal);
        self.current = caught;
        self.emit(
            OperationKind::RecordTaskError { scope },
            None,
            statement.range,
        );
        self.goto_if_open(next, ExitReason::Normal);
        self.current = next;
    }

    fn finish_collection(&mut self, name: &str, range: SourceRange) {
        let ty = self.local_types[name].clone();
        let name = name.to_owned();
        let buffer = self.emit_value(
            OperationKind::LoadLocal { name: name.clone() },
            ty.clone(),
            range,
        );
        // The runtime cannot yield or collect while publishing a finished
        // buffer. Keep the same cancellation points as the source loop.
        let value = self.emit_value(
            OperationKind::DataOperation {
                kind: crate::DataOperation::CollectionFinish,
                arguments: vec![buffer],
                argument_types: vec![ty.clone()],
                error: self.error_edge(),
                failure: self.failure,
                cleanup_chain: self.cleanup_chain(),
            },
            ty,
            range,
        );
        self.emit(OperationKind::StoreLocal { name, value }, None, range);
    }
    fn store_scoped_local(&mut self, name: String, ty: Type, value: ValueId, range: SourceRange) {
        self.local_types.insert(name.clone(), ty);
        let scope = *self.scope_stack.last().expect("active scope");
        self.scopes
            .iter_mut()
            .find(|info| info.scope == scope)
            .expect("scope info")
            .locals
            .push(name.clone());
        self.emit(
            OperationKind::DeclareLocal { name: name.clone() },
            None,
            range,
        );
        self.emit(OperationKind::StoreLocal { name, value }, None, range);
    }

    fn lower_for(
        &mut self,
        binding: &str,
        iterable: &Expression,
        body: &Block,
        range: SourceRange,
    ) {
        let outer = Block {
            statements: Vec::new(),
            range: SourceRange {
                instance: range.instance.wrapping_add(0x10000000),
                ..range
            },
        };
        self.lower_block_with_prelude(&outer, |this| {
            let ty = this.type_of(iterable);
            let Type::List(element) = &ty else {
                unreachable!("checked List iteration")
            };
            let list = this.lower_expression(iterable);
            let saved = format!("$for_list{}", list.0);
            let counter = format!("$for_index{}", list.0);
            let bound = format!("$for_length{}", list.0);
            this.store_scoped_local(saved.clone(), ty.clone(), list, range);
            this.emit_poll(PollReason::Allocation, range);
            let length = this.emit_value(
                OperationKind::DataOperation {
                    kind: crate::DataOperation::ListLength,
                    arguments: vec![list],
                    argument_types: vec![ty.clone()],
                    error: this.error_edge(),
                    failure: this.failure,
                    cleanup_chain: this.cleanup_chain(),
                },
                Type::Int,
                range,
            );
            this.store_scoped_local(bound.clone(), Type::Int, length, range);
            let zero = this.emit_value(
                OperationKind::Constant(ConstantValue::Int(0)),
                Type::Int,
                range,
            );
            this.store_scoped_local(counter.clone(), Type::Int, zero, range);
            let header = this.new_block(range);
            let enter = this.new_block(body.range);
            let step = this.new_block(range);
            let exit = this.new_block(range);
            this.goto_if_open(header, ExitReason::Normal);
            this.current = header;
            this.emit_poll(PollReason::LoopBackedge, range);
            let index = this.emit_value(
                OperationKind::LoadLocal {
                    name: counter.clone(),
                },
                Type::Int,
                range,
            );
            let length =
                this.emit_value(OperationKind::LoadLocal { name: bound }, Type::Int, range);
            let condition = this.emit_value(
                OperationKind::Binary {
                    operator: BinaryOperator::Less,
                    left: index,
                    right: length,
                    failure: None,
                    cleanup_chain: Vec::new(),
                },
                Type::Bool,
                range,
            );
            this.set_terminator(Terminator::Branch {
                condition,
                then_target: enter,
                else_target: exit,
            });
            this.loop_stack
                .push((step, exit, this.scope_stack.len(), this.cleanup_stack.len()));
            this.current = enter;
            this.lower_block_with_prelude(body, |this| {
                if binding != "_" {
                    let list = this.emit_value(
                        OperationKind::LoadLocal {
                            name: saved.clone(),
                        },
                        ty.clone(),
                        range,
                    );
                    let index = this.emit_value(
                        OperationKind::LoadLocal {
                            name: counter.clone(),
                        },
                        Type::Int,
                        range,
                    );
                    let value = this.emit_value(
                        OperationKind::ListElement {
                            operand: list,
                            index,
                        },
                        *element.clone(),
                        range,
                    );
                    this.store_scoped_local(binding.to_owned(), *element.clone(), value, range);
                }
            });
            if this.is_open(this.current) {
                this.emit_poll(PollReason::LoopBackedge, body.range);
            }
            this.goto_if_open(step, ExitReason::Continue);
            this.loop_stack.pop();
            this.current = step;
            let old = this.emit_value(
                OperationKind::LoadLocal {
                    name: counter.clone(),
                },
                Type::Int,
                range,
            );
            let one = this.emit_value(
                OperationKind::Constant(ConstantValue::Int(1)),
                Type::Int,
                range,
            );
            let next = this.emit_value(
                OperationKind::Binary {
                    operator: BinaryOperator::Add,
                    left: old,
                    right: one,
                    failure: Some(this.failure),
                    cleanup_chain: this.cleanup_chain(),
                },
                Type::Int,
                range,
            );
            this.emit(
                OperationKind::StoreLocal {
                    name: counter,
                    value: next,
                },
                None,
                range,
            );
            this.emit_poll(PollReason::LoopBackedge, range);
            this.goto_if_open(header, ExitReason::Continue);
            this.current = exit;
        });
    }

    fn lower_statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Throw(value) => self.lower_throw(value, statement.range),
            StatementKind::TryErrors {
                all,
                body,
                handlers,
                cleanup,
            } => {
                if *all {
                    self.lower_try_all(body, handlers, cleanup.as_ref(), statement.range);
                } else {
                    self.lower_error_handlers(body, handlers, cleanup.as_ref(), statement.range);
                }
            }
            StatementKind::Block(block) => self.lower_block(block),
            StatementKind::Local {
                name, initializer, ..
            } => {
                let ty = self.type_of(initializer);
                if self.all_scope.is_some() {
                    self.unbound_locals.insert(name.clone());
                }
                self.emit(
                    OperationKind::DeclareLocal { name: name.clone() },
                    None,
                    statement.range,
                );
                let value = self.lower_expression(initializer);
                self.local_types.insert(name.clone(), ty);
                if let Some(scope) = self.scope_stack.last()
                    && let Some(info) = self.scopes.iter_mut().find(|info| info.scope == *scope)
                {
                    info.locals.push(name.clone());
                }
                self.emit(
                    OperationKind::StoreLocal {
                        name: name.clone(),
                        value,
                    },
                    None,
                    statement.range,
                );
            }
            StatementKind::FieldAssign { base, field, value } => {
                let Type::Named(identity) = self.type_of(base) else {
                    unreachable!("checked state receiver")
                };
                let field = self.checked.source_types[&identity]
                    .fields
                    .iter()
                    .position(|(name, _)| name == field)
                    .unwrap() as u32;
                let operand = self.lower_expression(base);
                let value = self.lower_expression(value);
                self.emit(
                    OperationKind::StoreField {
                        operand,
                        identity,
                        field,
                        value,
                    },
                    None,
                    statement.range,
                );
            }
            StatementKind::Assign { name, value } => {
                let value = self.lower_expression(value);
                self.emit(
                    OperationKind::StoreLocal {
                        name: name.clone(),
                        value,
                    },
                    None,
                    statement.range,
                );
            }
            StatementKind::Expression(expression) => {
                self.lower_expression(expression);
            }
            StatementKind::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|expression| self.lower_expression(expression));
                self.set_terminator(Terminator::Return {
                    value,
                    cleanup_chain: self.control_cleanup_chain(0, 0),
                    release_scopes: self.scope_stack.iter().rev().copied().collect(),
                });
            }
            StatementKind::Break | StatementKind::Continue => {
                let (header, exit, scope_depth, cleanup_depth) = self
                    .loop_stack
                    .last()
                    .copied()
                    .expect("checked loop control");
                let is_continue = matches!(statement.kind, StatementKind::Continue);
                if is_continue {
                    self.emit_poll(PollReason::LoopBackedge, statement.range);
                }
                self.set_terminator(Terminator::Goto {
                    target: if is_continue { header } else { exit },
                    cleanup_chain: self.control_cleanup_chain(cleanup_depth, scope_depth),
                    release_scopes: self.scope_stack[scope_depth..]
                        .iter()
                        .rev()
                        .copied()
                        .collect(),
                    reason: if is_continue {
                        ExitReason::Continue
                    } else {
                        ExitReason::Break
                    },
                });
            }
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                let condition = self.lower_expression(condition);
                let then_target = self.new_block(then_block.range);
                let else_target = self.new_block(
                    else_branch
                        .as_ref()
                        .map_or(statement.range, |branch| branch.range),
                );
                let join = self.new_block(statement.range);
                self.set_terminator(Terminator::Branch {
                    condition,
                    then_target,
                    else_target,
                });
                self.current = then_target;
                self.lower_block(then_block);
                self.goto_if_open(join, ExitReason::Normal);
                self.current = else_target;
                if let Some(branch) = else_branch {
                    self.lower_statement(branch);
                }
                self.goto_if_open(join, ExitReason::Normal);
                self.current = join;
            }
            StatementKind::While { condition, body } => {
                let header = self.new_block(condition.range);
                let body_target = self.new_block(body.range);
                let exit = self.new_block(statement.range);
                self.goto_if_open(header, ExitReason::Normal);
                self.current = header;
                self.emit_poll(PollReason::LoopBackedge, condition.range);
                let condition = self.lower_expression(condition);
                self.set_terminator(Terminator::Branch {
                    condition,
                    then_target: body_target,
                    else_target: exit,
                });
                self.loop_stack.push((
                    header,
                    exit,
                    self.scope_stack.len(),
                    self.cleanup_stack.len(),
                ));
                self.current = body_target;
                self.lower_block(body);
                if self.is_open(self.current) {
                    self.emit_poll(PollReason::LoopBackedge, body.range);
                }
                self.goto_if_open(header, ExitReason::Continue);
                self.loop_stack.pop();
                self.current = exit;
            }
            StatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => {
                self.lower_for(binding, iterable, body, statement.range);
            }
            StatementKind::Switch { value, cases } => {
                let Type::Named(identity) = self.type_of(value) else {
                    unreachable!("checked closed enum switch")
                };
                let schema = self.checked.source_types[&identity].clone();
                let value = self.lower_expression(value);
                let saved = format!("$switch{}", value.0);
                self.local_types
                    .insert(saved.clone(), Type::Named(identity.clone()));
                let scope = self.scope_stack.last().expect("active scope");
                self.scopes
                    .iter_mut()
                    .find(|info| &info.scope == scope)
                    .expect("scope info")
                    .locals
                    .push(saved.clone());
                self.emit(
                    OperationKind::StoreLocal {
                        name: saved.clone(),
                        value,
                    },
                    None,
                    statement.range,
                );
                let join = self.new_block(statement.range);
                let targets: Vec<_> = cases
                    .iter()
                    .map(|case| self.new_block(case.range))
                    .collect();
                self.set_terminator(Terminator::Switch {
                    value,
                    identity: identity.clone(),
                    cases: cases
                        .iter()
                        .zip(&targets)
                        .map(|(case, target)| {
                            (
                                schema
                                    .variants
                                    .iter()
                                    .position(|(name, _)| Some(name) == case.path.last())
                                    .expect("checked variant")
                                    as u32,
                                *target,
                            )
                        })
                        .collect(),
                });
                for (case, target) in cases.iter().zip(targets) {
                    self.current = target;
                    let variant = schema
                        .variants
                        .iter()
                        .position(|(name, _)| Some(name) == case.path.last())
                        .expect("checked variant");
                    self.lower_block_with_prelude(&case.body, |builder| {
                        let operand = builder.emit_value(
                            OperationKind::LoadLocal {
                                name: saved.clone(),
                            },
                            Type::Named(identity.clone()),
                            case.range,
                        );
                        for (field, (name, (_, ty))) in case
                            .bindings
                            .iter()
                            .zip(&schema.variants[variant].1)
                            .enumerate()
                        {
                            let value = builder.emit_value(
                                OperationKind::EnumPayload {
                                    operand,
                                    identity: identity.clone(),
                                    variant: variant as u32,
                                    field: field as u32,
                                },
                                ty.clone(),
                                case.range,
                            );
                            builder.local_types.insert(name.clone(), ty.clone());
                            builder
                                .scopes
                                .last_mut()
                                .expect("case scope")
                                .locals
                                .push(name.clone());
                            builder.emit(
                                OperationKind::StoreLocal {
                                    name: name.clone(),
                                    value,
                                },
                                None,
                                case.range,
                            );
                        }
                    });
                    self.goto_if_open(join, ExitReason::Normal);
                }
                self.current = join;
            }
            StatementKind::TryCatch {
                body,
                binding,
                binding_range,
                handler,
                cleanup,
            } => {
                let handler = crate::ast::CatchClause::whole(binding, *binding_range, handler);
                self.lower_error_handlers(body, &[handler], cleanup.as_ref(), statement.range);
            }
            StatementKind::TaskScope { body, cleanup } => {
                let scope = statement.range;
                self.emit(
                    OperationKind::EnterTaskScope {
                        all: false,
                        scope,
                        body_scope: body.range,
                        cleanup_scope: cleanup.range,
                    },
                    None,
                    scope,
                );
                let join_range = SourceRange::new(
                    scope.file_id,
                    cleanup.range.end_byte as usize - 1,
                    cleanup.range.end_byte as usize,
                );
                self.task_joins.insert(scope, (scope, false));
                self.task_joins.insert(join_range, (scope, true));
                let cleanup = Block {
                    range: scope,
                    statements: vec![Statement {
                        range: cleanup.range,
                        kind: StatementKind::TryFinally {
                            body: cleanup.clone(),
                            cleanup: Block {
                                range: join_range,
                                statements: Vec::new(),
                            },
                        },
                    }],
                };
                self.lower_statement(&Statement {
                    range: scope,
                    kind: StatementKind::TryFinally {
                        body: body.clone(),
                        cleanup,
                    },
                });
            }
            StatementKind::TryFinally { body, cleanup } => {
                let previous_all = self.all_scope.take();
                let try_block = self.new_block(body.range);
                let cleanup_block = self.new_block(cleanup.range);
                let after = self.new_block(statement.range);
                self.emit(
                    OperationKind::EnterTry {
                        cleanup: cleanup_block,
                    },
                    None,
                    statement.range,
                );
                self.goto_if_open(try_block, ExitReason::Normal);
                self.cleanup_stack.push(cleanup_block);
                self.deferred_finally
                    .insert(body.range, vec![cleanup_block]);
                self.current = try_block;
                self.lower_block(body);
                self.goto_if_open(cleanup_block, ExitReason::Normal);
                self.cleanup_stack.pop();
                self.current = cleanup_block;
                self.emit(
                    OperationKind::LeaveTry {
                        cleanup: cleanup_block,
                    },
                    None,
                    cleanup.range,
                );
                self.lower_block(cleanup);
                self.goto_if_open(after, ExitReason::Normal);
                self.current = after;
                self.all_scope = previous_all;
            }
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => {
                let handlers = handler
                    .as_ref()
                    .map(|handler| {
                        crate::ast::CatchClause::whole(
                            &handler.binding,
                            handler.binding_range,
                            &handler.body,
                        )
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                self.lower_try_all(body, &handlers, cleanup.as_ref(), statement.range);
            }
        }
    }

    fn lower_expression(&mut self, expression: &Expression) -> ValueId {
        let coercion = self.checked.byte_coercions.get(&expression.range).copied();
        let ty = coercion.map_or_else(
            || {
                self.checked
                    .error_upcasts
                    .get(&expression.range)
                    .cloned()
                    .unwrap_or_else(|| self.type_of(expression))
            },
            |(from, _)| Type::FixedBytes(from),
        );
        let value = self.lower_uncoerced_expression(expression, ty);
        if let Some((from, to)) = coercion {
            self.emit_poll(PollReason::Allocation, expression.range);
            self.emit_value(
                OperationKind::BytesPad {
                    operand: value,
                    from,
                    to,
                },
                Type::FixedBytes(to),
                expression.range,
            )
        } else if self.checked.error_upcasts.contains_key(&expression.range) {
            self.emit_value(
                OperationKind::ErrorCast { operand: value },
                Type::Error,
                expression.range,
            )
        } else {
            value
        }
    }

    fn lower_uncoerced_expression(&mut self, expression: &Expression, ty: Type) -> ValueId {
        match &expression.kind {
            ExpressionKind::Is { value, .. } => {
                let operand = self.lower_expression(value);
                self.emit_value(
                    OperationKind::ErrorIs {
                        operand,
                        target: self.checked.error_tests[&expression.range].clone(),
                    },
                    Type::Bool,
                    expression.range,
                )
            }
            ExpressionKind::Function(_) => {
                unreachable!("anonymous functions converted before lowering")
            }
            ExpressionKind::Record(_) => unreachable!("record literals normalized before lowering"),
            ExpressionKind::Bind { .. } => unreachable!("bind converted before IR"),
            ExpressionKind::GenericCall { .. } | ExpressionKind::GenericReference { .. } => {
                unreachable!("generic syntax does not enter IR")
            }
            ExpressionKind::FormatString(parts) => {
                let parts: Vec<_> = parts
                    .iter()
                    .map(|part| (self.lower_expression(part), self.type_of(part)))
                    .collect();
                self.emit_poll(PollReason::Allocation, expression.range);
                let native_display = parts.iter().any(|(_, ty)| {
                    crate::sema::display_reads_native_record(&self.checked.source_types, ty)
                });
                self.emit_value(
                    OperationKind::FormatString {
                        parts,
                        error: native_display.then(|| self.error_edge()),
                        failure: native_display.then_some(self.failure),
                        cleanup_chain: if native_display {
                            self.cleanup_chain()
                        } else {
                            Vec::new()
                        },
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::Literal(literal) => {
                let value = match literal {
                    Literal::Bool(value) => ConstantValue::Bool(*value),
                    Literal::Int(value) => numeric_literal(*value as i128, &ty),
                    Literal::Unsigned(value) => numeric_literal(*value as i128, &ty),
                    Literal::DecimalInteger(spelling) => ConstantValue::Float(match ty.scalar() {
                        Some(crate::AbiScalar::F32) => {
                            f64::from(spelling.parse::<f32>().expect("decimal integer spelling"))
                        }
                        Some(crate::AbiScalar::F64) => {
                            spelling.parse::<f64>().expect("decimal integer spelling")
                        }
                        _ => unreachable!("out-of-range integer literals were rejected"),
                    }),
                    Literal::Float { value, spelling } => {
                        ConstantValue::Float(if ty.scalar() == Some(crate::AbiScalar::F32) {
                            f64::from(spelling.parse::<f32>().expect("validated decimal spelling"))
                        } else {
                            *value
                        })
                    }
                    Literal::String(value) => ConstantValue::String(value.clone()),
                    Literal::Bytes(value) => ConstantValue::Bytes(value.clone()),
                };
                if matches!(value, ConstantValue::String(_) | ConstantValue::Bytes(_)) {
                    self.emit_poll(PollReason::Allocation, expression.range);
                }
                self.emit_value(OperationKind::Constant(value), ty, expression.range)
            }
            ExpressionKind::Unit => self.emit_value(
                OperationKind::Constant(ConstantValue::Unit),
                ty,
                expression.range,
            ),
            ExpressionKind::Name(path) => {
                if (path.len() > 1
                    || self
                        .checked
                        .error_narrowings
                        .contains_key(&expression.range))
                    && self.local_types.contains_key(&path[0])
                {
                    let mut current_type = self.local_types[&path[0]].clone();
                    let mut operand = self.emit_value(
                        OperationKind::LoadLocal {
                            name: path[0].clone(),
                        },
                        current_type.clone(),
                        expression.range,
                    );
                    if let Some(narrowed) = self.checked.error_narrowings.get(&expression.range) {
                        current_type = narrowed.clone();
                        operand = self.emit_value(
                            OperationKind::ErrorCast { operand },
                            current_type.clone(),
                            expression.range,
                        );
                    }
                    for name in &path[1..] {
                        let concrete_error = matches!(&current_type, Type::Named(identity) if self.checked.source_types.get(identity).is_some_and(|schema|schema.kind=="error"));
                        if (current_type == Type::Error || (concrete_error && name == "source"))
                            && ["name", "message", "source", "cause"].contains(&name.as_str())
                        {
                            if concrete_error {
                                operand = self.emit_value(
                                    OperationKind::ErrorCast { operand },
                                    Type::Error,
                                    expression.range,
                                );
                            }
                            let field = ["name", "message", "source", "cause"]
                                .iter()
                                .position(|field| *field == name)
                                .expect("checked Error metadata")
                                as u32;
                            current_type = self.error_metadata_type(field);
                            self.emit_poll(PollReason::Allocation, expression.range);
                            operand = self.emit_value(
                                if field == 1 {
                                    OperationKind::ErrorMessage { error: operand }
                                } else {
                                    OperationKind::ErrorField {
                                        error: operand,
                                        field,
                                    }
                                },
                                current_type.clone(),
                                expression.range,
                            );
                        } else {
                            let Type::Named(identity) = &current_type else {
                                unreachable!("checked record field")
                            };
                            let fields = &self.checked.source_types[identity].fields;
                            let field = fields
                                .iter()
                                .position(|(field, _)| field == name)
                                .expect("checked field");
                            let result_type = fields[field].1.clone();
                            operand = self.emit_value(
                                OperationKind::Field {
                                    operand,
                                    identity: identity.clone(),
                                    field: field as u32,
                                    error: (self.checked.source_types[identity].kind
                                        == "native-record")
                                        .then(|| self.error_edge()),
                                    failure: (self.checked.source_types[identity].kind
                                        == "native-record")
                                        .then_some(self.failure),
                                    cleanup_chain: if self.checked.source_types[identity].kind
                                        == "native-record"
                                    {
                                        self.cleanup_chain()
                                    } else {
                                        Vec::new()
                                    },
                                },
                                result_type.clone(),
                                expression.range,
                            );
                            current_type = result_type;
                        }
                    }
                    operand
                } else if let Some(constant) =
                    self.checked.constant_references.get(&expression.range)
                {
                    self.emit_value(
                        OperationKind::Constant(ConstantValue::Int(constant.value as i64)),
                        ty,
                        expression.range,
                    )
                } else if matches!(ty, Type::FunctionReference(_))
                    && !self.local_types.contains_key(&path.join("."))
                {
                    self.emit_value(
                        OperationKind::FunctionReference {
                            target: self.checked.reference_targets[&expression.range][0].clone(),
                        },
                        ty,
                        expression.range,
                    )
                } else if matches!(ty, Type::StaticHandler(_)) {
                    self.emit_value(
                        OperationKind::StaticHandlerReference {
                            target: path.join("."),
                        },
                        ty,
                        expression.range,
                    )
                } else {
                    self.emit_value(
                        OperationKind::LoadLocal {
                            name: path.join("."),
                        },
                        ty,
                        expression.range,
                    )
                }
            }
            ExpressionKind::Unary { operator, operand } => {
                if *operator == UnaryOperator::Negate && ty.is_integer() {
                    let signed = match &operand.kind {
                        ExpressionKind::Literal(Literal::Int(value)) => Some(-(*value as i128)),
                        ExpressionKind::Literal(Literal::Unsigned(value)) => {
                            Some(-(*value as i128))
                        }
                        _ => None,
                    };
                    if let Some(value) = signed {
                        return self.emit_value(
                            OperationKind::Constant(ConstantValue::Int(value as i64)),
                            ty,
                            expression.range,
                        );
                    }
                }
                let operand = self.lower_expression(operand);
                self.emit_value(
                    OperationKind::Unary {
                        operator: *operator,
                        operand,
                        failure: (*operator == UnaryOperator::Negate && ty.is_integer())
                            .then_some(self.failure),
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.lower_expression(left);
                if matches!(operator, BinaryOperator::And | BinaryOperator::Or) {
                    return self.lower_short_circuit(*operator, left, right, expression.range);
                }
                let right = self.lower_expression(right);
                let can_fail = ty.is_integer()
                    && matches!(
                        operator,
                        BinaryOperator::Add
                            | BinaryOperator::Subtract
                            | BinaryOperator::Multiply
                            | BinaryOperator::Divide
                            | BinaryOperator::Remainder
                    );
                self.emit_value(
                    OperationKind::Binary {
                        operator: *operator,
                        left,
                        right,
                        failure: can_fail.then_some(self.failure),
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::Call { target, arguments } => {
                if let Some(kind) = self.checked.task_controls.get(&expression.range).copied() {
                    let Arguments::Positional(values) = arguments else {
                        unreachable!("checked task control")
                    };
                    let future = values.first().map(|v| self.lower_expression(v));
                    self.emit_poll(PollReason::ManagedCall, expression.range);
                    let result = self.emit_value(
                        OperationKind::TaskControl { kind, future },
                        ty,
                        expression.range,
                    );
                    self.emit_poll(PollReason::ManagedCall, expression.range);
                    return result;
                }
                if let Some(mut kind) = self.checked.data_operations.get(&expression.range).copied()
                {
                    let Arguments::Positional(values) = arguments else {
                        unreachable!("checked data operation")
                    };
                    let arguments = values.iter().map(|v| self.lower_expression(v)).collect();
                    self.emit_poll(PollReason::Allocation, expression.range);
                    let argument_types = values.iter().map(|v| self.type_of(v)).collect();
                    if self.buffered_updates.contains(&expression.range) {
                        kind = match kind {
                            crate::DataOperation::ListAppend => {
                                crate::DataOperation::ListBufferAppend
                            }
                            crate::DataOperation::MapInsert => {
                                crate::DataOperation::MapBufferInsert
                            }
                            _ => unreachable!("checked collection update"),
                        };
                    }
                    return self.emit_value(
                        OperationKind::DataOperation {
                            kind,
                            arguments,
                            argument_types,
                            error: self.error_edge(),
                            failure: self.failure,
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    );
                }
                if let Some(newline) = self.checked.stdout_calls.get(&expression.range).copied() {
                    let Arguments::Positional(values) = arguments else {
                        unreachable!("checked stdout")
                    };
                    let operand = self.lower_expression(&values[0]);
                    self.emit_poll(PollReason::NativeCall, expression.range);
                    return self.emit_value(
                        OperationKind::Stdout {
                            operand,
                            ty: self.type_of(&values[0]),
                            newline,
                            error: self.error_edge(),
                            failure: self.failure,
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    );
                }
                if let Some(kind) = self.checked.core_lengths.get(&expression.range).copied() {
                    let Arguments::Positional(values) = arguments else {
                        unreachable!("checked core length")
                    };
                    let operand = self.lower_expression(&values[0]);
                    return self.emit_value(
                        OperationKind::CoreLength { operand, kind },
                        ty,
                        expression.range,
                    );
                }
                if target.len() == 1
                    && crate::AbiScalar::from_name(&target[0])
                        .is_some_and(|scalar| scalar != crate::AbiScalar::Bool)
                    && !self.checked.indirect_calls.contains_key(&expression.range)
                {
                    let Arguments::Positional(values) = arguments else {
                        unreachable!("validated numeric conversion")
                    };
                    let source = self.type_of(&values[0]);
                    let operand = self.lower_expression(&values[0]);
                    if source == ty {
                        return operand;
                    }
                    return self.emit_value(
                        OperationKind::NumericConvert {
                            operand,
                            source,
                            failure: self.failure,
                            error: self.error_edge(),
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    );
                }
                if let Some(targets) = self.checked.indirect_calls.get(&expression.range) {
                    let callee_type = self
                        .local_types
                        .get(&target.join("."))
                        .expect("checked function reference local")
                        .clone();
                    let callee = self.emit_value(
                        OperationKind::LoadLocal {
                            name: target.join("."),
                        },
                        callee_type,
                        expression.range,
                    );
                    let arguments = self.lower_arguments(arguments);
                    self.emit_poll(PollReason::ManagedCall, expression.range);
                    return self.emit_value(
                        OperationKind::IndirectCall {
                            callee,
                            targets: targets.clone(),
                            arguments,
                            failure: self.failure,
                            error: self.error_edge(),
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    );
                }
                let constructor_labels = match arguments {
                    Arguments::Named(values) => values
                        .iter()
                        .map(|(name, _, _)| name.clone())
                        .collect::<Vec<_>>(),
                    Arguments::Positional(_) => Vec::new(),
                };
                let arguments = self.lower_arguments(arguments);
                if let Type::Named(identity) = &ty
                    && target.join(".") == *identity
                    && self
                        .checked
                        .source_types
                        .get(identity)
                        .is_some_and(|schema| schema.kind == "error")
                {
                    return self.lower_error_constructor(
                        identity,
                        &constructor_labels,
                        &arguments,
                        expression.range,
                    );
                }
                if let Some(signature) = self.checked.source_native_calls.get(&expression.range) {
                    self.emit_poll(PollReason::NativeCall, expression.range);
                    if signature.is_async {
                        self.emit_value(
                            OperationKind::SourceNativeCall {
                                call: None,
                                signature: signature.clone(),
                                arguments,
                                failure: self.failure,
                                error: self.error_edge(),
                                cleanup_chain: self.cleanup_chain(),
                            },
                            ty,
                            expression.range,
                        )
                    } else {
                        let call = NativeCallId(self.next_native_call);
                        self.next_native_call += 1;
                        let crate::ast::Arguments::Positional(source_arguments) =
                            expression_arguments(expression)
                        else {
                            unreachable!("checked positional call")
                        };
                        let callbacks = signature
                            .parameters
                            .iter()
                            .enumerate()
                            .filter(|(_, parameter)| {
                                matches!(parameter, crate::SourceAbiType::Callback { .. })
                            })
                            .map(|(index, _)| CallbackArgumentPlan {
                                index,
                                capture: self
                                    .checked
                                    .native_captures
                                    .get(&source_arguments[index].range)
                                    .cloned(),
                                targets: self
                                    .checked
                                    .reference_targets
                                    .get(&source_arguments[index].range)
                                    .cloned()
                                    .unwrap_or_default(),
                            })
                            .collect();
                        self.emit(
                            OperationKind::PrepareNativeCall {
                                call,
                                signature: signature.clone(),
                                arguments: arguments.clone(),
                                retained: arguments.clone(),
                                callbacks,
                                failure: self.failure,
                                error: self.error_edge(),
                                cleanup_chain: self.cleanup_chain(),
                            },
                            None,
                            expression.range,
                        );
                        self.active_native_calls.push(call);
                        self.emit(
                            OperationKind::SourceNativeCall {
                                call: Some(call),
                                signature: signature.clone(),
                                arguments,
                                failure: self.failure,
                                error: ErrorEdge {
                                    target: self.failure,
                                    cleanup_chain: self.cleanup_chain(),
                                    release_scopes: self
                                        .scope_stack
                                        .iter()
                                        .rev()
                                        .copied()
                                        .collect(),
                                    release_native_calls: vec![call],
                                },
                                cleanup_chain: self.cleanup_chain(),
                            },
                            None,
                            expression.range,
                        );
                        self.emit(
                            OperationKind::CloseNativeCall { call },
                            None,
                            expression.range,
                        );
                        self.emit(
                            OperationKind::SettleNativeAdmissions {
                                call,
                                origin: ResumeDomain::Origin,
                            },
                            None,
                            expression.range,
                        );
                        let value = self.emit_value(
                            OperationKind::ReadNativeResult {
                                call,
                                failure: self.failure,
                                error: self.error_edge(),
                                cleanup_chain: self.cleanup_chain(),
                            },
                            ty,
                            expression.range,
                        );
                        self.emit(
                            OperationKind::ReleaseNativeCall { call },
                            None,
                            expression.range,
                        );
                        self.active_native_calls.pop();
                        value
                    }
                } else if let Some(call_class) = self.checked.native_calls.get(&expression.range) {
                    self.emit_poll(PollReason::NativeCall, expression.range);
                    let transition = match call_class {
                        NativeCallClass::NativeSafe => NativeTransition::NativeSafe,
                        NativeCallClass::OwnerThreadSync => NativeTransition::OwnerThreadSync,
                    };
                    self.emit_value(
                        OperationKind::NativeCall {
                            target: target.join("."),
                            arguments,
                            failure: self.failure,
                            error: self.error_edge(),
                            transition,
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    )
                } else if self.function_named(&target[0]) {
                    self.emit_poll(PollReason::ManagedCall, expression.range);
                    self.emit_value(
                        OperationKind::ManagedCall {
                            target: target.join("."),
                            arguments,
                            failure: self.failure,
                            error: self.error_edge(),
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    )
                } else {
                    let Type::Named(identity) = &ty else {
                        unreachable!("checked nominal constructor")
                    };
                    let schema = &self.checked.source_types[identity];
                    let variant = (schema.kind == "enum").then(|| {
                        schema
                            .variants
                            .iter()
                            .position(|(name, _)| Some(name) == target.last())
                            .expect("checked variant") as u32
                    });
                    let fields = variant.map_or(&schema.fields, |variant| {
                        &schema.variants[variant as usize].1
                    });
                    // Evaluate in source order, then arrange existing SSA values for the layout.
                    let arguments = fields
                        .iter()
                        .map(|(name, _)| {
                            arguments[constructor_labels
                                .iter()
                                .position(|label| label == name)
                                .expect("checked argument")]
                        })
                        .collect();
                    let target = identity.clone();
                    self.emit_poll(PollReason::Allocation, expression.range);
                    self.emit_value(
                        OperationKind::Construct {
                            target,
                            variant,
                            arguments,
                            failure: self.failure,
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    )
                }
            }
            ExpressionKind::RecordUpdate { base, fields } => {
                let operand = self.lower_expression(base);
                let Type::Named(identity) = &ty else {
                    unreachable!("checked record update")
                };
                let mut replacements = std::collections::HashMap::new();
                for (name, value, _) in fields {
                    replacements.insert(name, self.lower_expression(value));
                }
                let layout = self.checked.source_types[identity].fields.clone();
                let mut arguments = Vec::with_capacity(layout.len());
                for (index, (name, field_ty)) in layout.into_iter().enumerate() {
                    arguments.push(if let Some(value) = replacements.get(&name) {
                        *value
                    } else {
                        self.emit_value(
                            OperationKind::Field {
                                operand,
                                identity: identity.clone(),
                                field: index as u32,
                                error: None,
                                failure: None,
                                cleanup_chain: Vec::new(),
                            },
                            field_ty,
                            expression.range,
                        )
                    });
                }
                self.emit_poll(PollReason::Allocation, expression.range);
                self.emit_value(
                    OperationKind::Construct {
                        target: identity.clone(),
                        variant: None,
                        arguments,
                        failure: self.failure,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::List(values) => {
                let arguments = values
                    .iter()
                    .map(|value| self.lower_expression(value))
                    .collect();
                self.emit_poll(PollReason::Allocation, expression.range);
                self.emit_value(
                    OperationKind::Construct {
                        target: "List".to_owned(),
                        variant: None,
                        arguments,
                        failure: self.failure,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::Map(entries) => {
                let arguments = entries
                    .iter()
                    .flat_map(|(key, value)| {
                        [self.lower_expression(key), self.lower_expression(value)]
                    })
                    .collect();
                self.emit_poll(PollReason::Allocation, expression.range);
                self.emit_value(
                    OperationKind::Construct {
                        target: "Map".to_owned(),
                        variant: None,
                        arguments,
                        failure: self.failure,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::Await(future) => {
                if let Some(call) = self.checked.supplant_calls.get(&future.range) {
                    let ExpressionKind::Call { arguments, .. } = &future.kind else {
                        unreachable!("direct await checked")
                    };
                    let callee = call.reference.as_ref().map(|name| {
                        self.emit_value(
                            OperationKind::LoadLocal { name: name.clone() },
                            Type::FunctionReference(Box::new(call.signature.clone())),
                            future.range,
                        )
                    });
                    let Arguments::Positional(source_arguments) = arguments else {
                        unreachable!("checked supplant arguments")
                    };
                    let callbacks = call
                        .signature
                        .parameters
                        .iter()
                        .enumerate()
                        .filter(|(_, ty)| matches!(ty, Type::FunctionReference(_)))
                        .map(|(index, _)| CallbackArgumentPlan {
                            index,
                            capture: self
                                .checked
                                .native_captures
                                .get(&source_arguments[index].range)
                                .cloned(),
                            targets: self
                                .checked
                                .reference_targets
                                .get(&source_arguments[index].range)
                                .cloned()
                                .unwrap_or_default(),
                        })
                        .collect();
                    let arguments = self.lower_arguments(arguments);
                    let mut retained = arguments.clone();
                    retained.extend(callee);
                    self.emit_poll(PollReason::Await, expression.range);
                    let reservation = ReservationId(self.next_reservation);
                    self.next_reservation += 1;
                    self.emit(
                        OperationKind::ReserveSupplant {
                            reservation,
                            callee,
                            targets: call.targets.clone(),
                            arguments,
                            retained,
                            signature: call.signature.clone(),
                            callbacks,
                        },
                        None,
                        expression.range,
                    );
                    self.emit(
                        OperationKind::StartSupplant {
                            reservation,
                            entry_domain: NativeEntryDomain::ProcessMain,
                            suspend_before_entry: true,
                        },
                        None,
                        expression.range,
                    );
                    return self.emit_value(
                        OperationKind::SettleSupplant {
                            reservation,
                            release: true,
                            failure: self.failure,
                            error: self.error_edge(),
                            cleanup_chain: self.cleanup_chain(),
                        },
                        ty,
                        expression.range,
                    );
                }
                let consume = match &future.kind {
                    ExpressionKind::Spawn { .. } => true,
                    ExpressionKind::Call { target, .. } => {
                        self.checked
                            .source_native_calls
                            .get(&future.range)
                            .is_some_and(|s| s.is_async)
                            || if let Some(targets) = self.checked.indirect_calls.get(&future.range)
                            {
                                !targets.is_empty()
                                    && targets.iter().all(|target| {
                                        self.checked.functions.iter().any(|f| {
                                            f.declaration.name == *target && f.declaration.is_async
                                        })
                                    })
                            } else {
                                self.checked.functions.iter().any(|f| {
                                    f.declaration.name == target.join(".") && f.declaration.is_async
                                })
                            }
                    }
                    _ => false,
                };
                let future = self.lower_expression(future);
                self.emit_poll(PollReason::Await, expression.range);
                self.emit_value(
                    OperationKind::Await {
                        future,
                        consume,
                        failure: self.failure,
                        error: self.error_edge(),
                        ready: AwaitReadyPath::ContinueInline,
                        pending: AwaitPendingPath::SuspendAndResume,
                        resume_domain: ResumeDomain::Origin,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
            ExpressionKind::Spawn { target, arguments } => {
                let arguments = self.lower_arguments(arguments);
                self.emit_poll(PollReason::Spawn, expression.range);
                self.emit_value(
                    OperationKind::Spawn {
                        target: target.join("."),
                        arguments,
                        failure: self.failure,
                        admission: SpawnAdmission::Required,
                        settle_domain: ResumeDomain::Origin,
                        cleanup_chain: self.cleanup_chain(),
                    },
                    ty,
                    expression.range,
                )
            }
        }
    }

    fn lower_arguments(&mut self, arguments: &Arguments) -> Vec<ValueId> {
        match arguments {
            Arguments::Positional(values) => values
                .iter()
                .map(|value| self.lower_expression(value))
                .collect(),
            Arguments::Named(values) => values
                .iter()
                .map(|(_, value, _)| self.lower_expression(value))
                .collect(),
        }
    }

    fn expression_local(&mut self, name: String, ty: Type) {
        self.local_types.insert(name.clone(), ty);
        let scope = *self.scope_stack.last().expect("expression scope");
        self.scopes
            .iter_mut()
            .find(|info| info.scope == scope)
            .expect("expression scope info")
            .locals
            .push(name);
    }

    fn lower_short_circuit(
        &mut self,
        operator: BinaryOperator,
        left: ValueId,
        right: &Expression,
        range: SourceRange,
    ) -> ValueId {
        // IR values are block-local. Preserve enclosing expression operands,
        // including managed roots, before introducing expression-level edges.
        let live = self.blocks[self.current.0 as usize]
            .operations
            .iter()
            .filter_map(|operation| operation.result.clone())
            .collect::<Vec<_>>();
        let consumed = self.blocks[self.current.0 as usize]
            .operations
            .iter()
            .filter_map(|operation| match operation.kind {
                OperationKind::Await {
                    future,
                    consume: true,
                    ..
                } => Some(future),
                _ => None,
            })
            .collect::<HashSet<_>>();
        for value in live {
            let active = self.value_definitions[&value.id]
                .2
                .is_none_or(|scope| self.scope_stack.contains(&scope));
            if active
                && !consumed.contains(&value.id)
                && !self.expression_spills.contains_key(&value.id)
            {
                let name = format!("$short_value{}", value.id.0);
                self.expression_local(name.clone(), value.ty);
                self.emit(
                    OperationKind::StoreLocal {
                        name: name.clone(),
                        value: value.id,
                    },
                    None,
                    range,
                );
                self.expression_spills.insert(value.id, name);
            }
        }
        let name = format!("$short_result{}", left.0);
        self.expression_local(name.clone(), Type::Bool);
        self.emit(
            OperationKind::StoreLocal {
                name: name.clone(),
                value: left,
            },
            None,
            range,
        );
        let rhs = self.new_block(right.range);
        let join = self.new_block(range);
        let (then_target, else_target) = if operator == BinaryOperator::And {
            (rhs, join)
        } else {
            (join, rhs)
        };
        self.set_terminator(Terminator::Branch {
            condition: left,
            then_target,
            else_target,
        });
        self.current = rhs;
        let value = self.lower_expression(right);
        self.emit(
            OperationKind::StoreLocal {
                name: name.clone(),
                value,
            },
            None,
            right.range,
        );
        self.goto_if_open(join, ExitReason::Normal);
        self.current = join;
        self.emit_value(OperationKind::LoadLocal { name }, Type::Bool, range)
    }

    fn expression_input(&mut self, value: ValueId, range: SourceRange) -> ValueId {
        let Some((block, ty, _)) = self.value_definitions.get(&value).cloned() else {
            return value;
        };
        if block == self.current {
            return value;
        }
        if let Some(reloaded) = self.expression_reloads.get(&(self.current, value)) {
            return *reloaded;
        }
        let name = self
            .expression_spills
            .get(&value)
            .expect("expression operand across blocks")
            .clone();
        let reloaded = self.emit_value(OperationKind::LoadLocal { name }, ty, range);
        self.expression_reloads
            .insert((self.current, value), reloaded);
        reloaded
    }

    fn type_of(&self, expression: &Expression) -> Type {
        self.checked
            .expression_type(expression.range)
            .cloned()
            .unwrap_or(Type::Never)
    }

    fn function_named(&self, name: &str) -> bool {
        self.checked
            .functions
            .iter()
            .any(|function| function.declaration.name == name)
    }

    fn emit_poll(&mut self, reason: PollReason, range: SourceRange) {
        let cleanup_chain = self.cleanup_chain();
        self.emit(
            OperationKind::CancellationPoll { cleanup_chain },
            None,
            range,
        );
        self.emit_gc(reason, range);
    }

    fn emit_gc(&mut self, reason: PollReason, range: SourceRange) {
        let mut roots = self.blocks[self.current.0 as usize]
            .operations
            .iter()
            .filter_map(|operation| operation.result.as_ref())
            .filter(|result| result.ty.is_managed_reference())
            .map(|result| StackRoot::Value {
                value: result.id,
                ty: result.ty.clone(),
            })
            .collect::<Vec<_>>();
        let mut locals = self
            .local_types
            .iter()
            .filter(|(_, ty)| ty.is_managed_reference())
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect::<Vec<_>>();
        locals.sort_by(|left, right| left.0.cmp(&right.0));
        roots.extend(
            locals
                .into_iter()
                .map(|(name, ty)| StackRoot::Local { name, ty }),
        );
        self.emit(
            OperationKind::GcSafepoint {
                reason,
                stack_map: StackMap { roots },
            },
            None,
            range,
        );
    }

    fn emit_value(&mut self, kind: OperationKind, ty: Type, range: SourceRange) -> ValueId {
        if let OperationKind::LoadLocal { name } = &kind
            && self.unbound_locals.contains(name)
        {
            self.emit(
                OperationKind::CheckLocal {
                    name: name.clone(),
                    error: self.error_edge(),
                    failure: self.failure,
                    cleanup_chain: self.cleanup_chain(),
                },
                None,
                range,
            );
        }
        let id = ValueId(self.next_value);
        self.next_value += 1;
        self.value_definitions.insert(
            id,
            (self.current, ty.clone(), self.scope_stack.last().copied()),
        );
        self.emit(kind, Some(TypedValue { id, ty }), range);
        id
    }

    fn emit(&mut self, mut kind: OperationKind, result: Option<TypedValue>, range: SourceRange) {
        let will_reload = operation_inputs(&kind).into_iter().any(|value| {
            self.value_definitions
                .get(&value)
                .is_some_and(|(block, _, _)| *block != self.current)
                && !self.expression_reloads.contains_key(&(self.current, value))
        });
        // Operand reloads must precede the call/allocation's immediate poll.
        // Rebuild the stack map after reloads, without adding a second poll.
        let poll = if will_reload {
            let operations = &mut self.blocks[self.current.0 as usize].operations;
            if let Some(Operation {
                kind: OperationKind::GcSafepoint { reason, .. },
                range,
                ..
            }) = operations.last()
            {
                let reason = *reason;
                let poll_range = *range;
                operations.pop();
                let cancellation = operations.last().is_some_and(|operation| {
                    operation.range == poll_range
                        && matches!(operation.kind, OperationKind::CancellationPoll { .. })
                });
                if cancellation {
                    operations.pop();
                }
                Some((reason, poll_range, cancellation))
            } else {
                None
            }
        } else {
            None
        };
        for value in operation_inputs_mut(&mut kind) {
            *value = self.expression_input(*value, range);
        }
        if let Some((reason, range, cancellation)) = poll {
            if cancellation {
                self.emit_poll(reason, range);
            } else {
                self.emit_gc(reason, range);
            }
        }
        let unwind = if operation_failure_target(&kind).is_some()
            || matches!(kind, OperationKind::CancellationPoll { .. })
        {
            Some(UnwindPlan {
                finally: operation_cleanup_chain(&kind).unwrap_or_default().to_vec(),
                release_scopes: self.scope_stack.iter().rev().copied().collect(),
                release_native_calls: self.active_native_calls.iter().rev().copied().collect(),
            })
        } else {
            None
        };
        self.block_mut(self.current).operations.push(Operation {
            result,
            kind,
            range,
            unwind,
        });
    }

    fn new_block(&mut self, range: SourceRange) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock {
            id,
            operations: Vec::new(),
            entry_scopes: self.scope_stack.clone(),
            terminator: Terminator::Pending,
            range,
        });
        id
    }

    fn set_terminator(&mut self, terminator: Terminator) {
        self.block_mut(self.current).terminator = terminator;
    }

    fn goto_if_open(&mut self, target: BlockId, reason: ExitReason) {
        if self.is_open(self.current) {
            self.set_terminator(Terminator::Goto {
                target,
                cleanup_chain: Vec::new(),
                release_scopes: Vec::new(),
                reason,
            });
        }
    }

    fn error_edge(&self) -> ErrorEdge {
        if let Some((target, depth, scope_depth)) = self.catch_stack.last() {
            ErrorEdge {
                release_native_calls: self.active_native_calls.iter().rev().copied().collect(),
                target: *target,
                release_scopes: self.scope_stack[*scope_depth..]
                    .iter()
                    .rev()
                    .copied()
                    .collect(),
                cleanup_chain: self.cleanup_stack[*depth..].iter().rev().copied().collect(),
            }
        } else {
            ErrorEdge {
                release_native_calls: self.active_native_calls.iter().rev().copied().collect(),
                target: self.failure,
                release_scopes: self.scope_stack.iter().rev().copied().collect(),
                cleanup_chain: self.cleanup_chain(),
            }
        }
    }

    fn control_cleanup_chain(&self, minimum: usize, target_scope_depth: usize) -> Vec<BlockId> {
        let mut chain = Vec::new();
        for depth in (minimum..=self.cleanup_stack.len()).rev() {
            for (epilogue, cleanup_depth, scope_depth) in self.control_epilogues.iter().rev() {
                if *cleanup_depth == depth && *scope_depth > target_scope_depth {
                    chain.push(*epilogue);
                }
            }
            if depth > minimum {
                chain.push(self.cleanup_stack[depth - 1]);
            }
        }
        chain
    }
    fn cleanup_chain(&self) -> Vec<BlockId> {
        self.cleanup_stack.iter().rev().copied().collect()
    }

    fn is_open(&self, id: BlockId) -> bool {
        matches!(self.blocks[id.0 as usize].terminator, Terminator::Pending)
    }

    fn block_mut(&mut self, id: BlockId) -> &mut BasicBlock {
        &mut self.blocks[id.0 as usize]
    }
}

pub fn verify_module(module: &IrModule) -> Result<(), Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut names = HashMap::new();
    for function in &module.functions {
        if let Some(previous) = names.insert(&function.name, function.range) {
            diagnostics.push(
                ir_error(
                    function.range,
                    format!("duplicate function name `{}`", function.name),
                )
                .with_secondary(previous, "first function is here"),
            );
        }
        verify_function(module, function, &mut diagnostics);
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn verify_task_groups(function: &IrFunction, diagnostics: &mut Vec<Diagnostic>) {
    let all_groups = function
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .filter_map(|op| {
            if let OperationKind::EnterErrorGroup { scope } = op.kind {
                Some(scope)
            } else {
                None
            }
        })
        .collect::<HashSet<_>>();
    let mut groups = HashMap::new();
    let mut outcomes: HashMap<SourceRange, Vec<(BlockId, &BasicBlock)>> = HashMap::new();
    let mut joins: HashMap<SourceRange, Vec<(bool, &BasicBlock, usize)>> = HashMap::new();
    for block in &function.blocks {
        for (index, operation) in block.operations.iter().enumerate() {
            match operation.kind {
                OperationKind::RecordTaskError { scope } => {
                    if !all_groups.contains(&scope) || index != 0 || operation.result.is_some() {
                        diagnostics.push(ir_error(
                            operation.range,
                            "Error collection requires a lexical Error group catch entry",
                        ));
                    }
                }
                OperationKind::TakeTaskErrors { scope } => {
                    if !all_groups.contains(&scope)
                        || operation.result.as_ref().map(|v| &v.ty)
                            != Some(&Type::List(Box::new(Type::Error)))
                        || !function.is_async
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "group catch requires List<Error> after an all-group join",
                        ));
                    }
                }
                OperationKind::EnterTaskScope {
                    scope,
                    body_scope,
                    cleanup_scope,
                    ..
                } => {
                    if groups.insert(scope, (body_scope, cleanup_scope)).is_some() {
                        diagnostics.push(ir_error(operation.range, "duplicate task group entry"));
                    }
                }
                OperationKind::TaskScopeOutcome { scope, cleanup, .. } => {
                    outcomes.entry(scope).or_default().push((cleanup, block));
                }
                OperationKind::JoinTaskScope { scope, cleanup } => {
                    joins
                        .entry(scope)
                        .or_default()
                        .push((cleanup, block, index));
                    if !block.operations[..index].iter().any(|op| matches!(op.kind, OperationKind::LeaveTry { cleanup } if cleanup == block.id))
                        || !block.operations[..index].iter().all(|op| matches!(op.kind,
                            OperationKind::LeaveTry { .. } | OperationKind::EnterScope { .. } | OperationKind::GcSafepoint { .. }))
                        || index == 0 || !matches!(block.operations[index - 1].kind, OperationKind::GcSafepoint { .. }) {
                        diagnostics.push(ir_error(operation.range, "task group join must start a rooted cleanup before user effects"));
                    }
                }
                _ => {}
            }
        }
    }
    for (scope, (body, cleanup)) in &groups {
        let boundaries = joins.remove(scope).unwrap_or_default();
        if boundaries.len() != 2 || boundaries.iter().filter(|b| b.0).count() != 1 {
            diagnostics.push(ir_error(
                *scope,
                "task group requires exactly one body join and one cleanup join",
            ));
            continue;
        }
        let checks = outcomes.remove(scope).unwrap_or_default();
        let body_join = boundaries.iter().find(|b| !b.0).unwrap().1.id;
        if checks.len() != 1
            || checks[0].0 != body_join
            || !checks[0].1.entry_scopes.contains(scope)
        {
            diagnostics.push(ir_error(
                *scope,
                "task group requires one outcome edge after both joins",
            ));
        }
        for (cleaning, block, _) in boundaries {
            let lexical = if cleaning { cleanup } else { body };
            if !function
                .scopes
                .iter()
                .any(|info| info.scope == *lexical && info.deferred_finally == [block.id])
            {
                diagnostics.push(ir_error(
                    *scope,
                    "task group join is missing from its lexical unwind plan",
                ));
            }
        }
    }
    for scope in outcomes.keys() {
        diagnostics.push(ir_error(*scope, "task group outcome has no matching entry"));
    }
    for scope in joins.keys() {
        diagnostics.push(ir_error(*scope, "task group join has no matching entry"));
    }
}

fn verify_function(module: &IrModule, function: &IrFunction, diagnostics: &mut Vec<Diagnostic>) {
    verify_task_groups(function, diagnostics);
    errors::verify_groups(function, diagnostics);
    if function
        .callback_fallback
        .as_ref()
        .is_some_and(|fallback| function.is_async || !fallback.valid_for(&function.result_type))
    {
        diagnostics.push(ir_error(
            function.range,
            "callback fallback does not match the synchronous result ABI",
        ));
    }
    let block_ids: HashSet<_> = function.blocks.iter().map(|block| block.id).collect();
    if block_ids.len() != function.blocks.len() {
        diagnostics.push(ir_error(function.range, "basic block identity was reused"));
    }
    if !block_ids.contains(&function.entry) || !block_ids.contains(&function.failure) {
        diagnostics.push(ir_error(
            function.range,
            "function entry or failure block is missing",
        ));
    }
    let scope_info: HashMap<_, _> = function
        .scopes
        .iter()
        .map(|info| (info.scope, info))
        .collect();
    if scope_info.len() != function.scopes.len() {
        diagnostics.push(ir_error(
            function.range,
            "lexical scope identity was reused",
        ));
    }
    for info in &function.scopes {
        verify_chain(&info.deferred_finally, &block_ids, info.scope, diagnostics);
        if info.deferred_finally.iter().any(|target| !function.blocks.iter().any(|block| block.id == *target
            && block.operations.iter().any(|op| matches!(op.kind, OperationKind::LeaveTry { cleanup } if cleanup == *target)))) {
            diagnostics.push(ir_error(info.scope, "deferred scope release does not name a finally entry"));
        }
        if info.parent.is_some_and(|parent| {
            !scope_info.contains_key(&parent)
                || parent.file_id != info.scope.file_id
                || parent.start_byte > info.scope.start_byte
                || parent.end_byte < info.scope.end_byte
        }) {
            diagnostics.push(ir_error(info.scope, "lexical scope has an invalid parent"));
        }
    }
    let mut values = HashSet::new();
    let mut value_types = HashMap::new();
    let mut reservations: HashMap<ReservationId, (Type, BlockId, u8)> = HashMap::new();
    let mut native_calls: HashMap<
        NativeCallId,
        (crate::SourceNativeSignature, Vec<ValueId>, BlockId, u8),
    > = HashMap::new();
    for block in &function.blocks {
        let mut active_scopes = block.entry_scopes.clone();
        let mut active_native_calls = Vec::new();
        if !scope_chain_valid(&active_scopes, &scope_info) {
            diagnostics.push(ir_error(
                block.range,
                "basic block entry has an invalid lexical scope chain",
            ));
        }
        let mut local_values = HashSet::new();
        for (index, operation) in block.operations.iter().enumerate() {
            for input in operation_inputs(&operation.kind) {
                if !local_values.contains(&input) {
                    diagnostics.push(ir_error(
                        operation.range,
                        format!("value {:?} is used before a dominating definition", input),
                    ));
                }
            }
            if let Some(result) = &operation.result {
                if !values.insert(result.id) {
                    diagnostics.push(ir_error(
                        operation.range,
                        format!("value {:?} is defined more than once", result.id),
                    ));
                }
                local_values.insert(result.id);
                value_types.insert(result.id, result.ty.clone());
            }
            if let Some(chain) = operation_cleanup_chain(&operation.kind) {
                verify_chain(chain, &block_ids, operation.range, diagnostics);
            }
            if matches!(
                operation.kind,
                OperationKind::PrepareNativeCall { .. }
                    | OperationKind::CloseNativeCall { .. }
                    | OperationKind::SettleNativeAdmissions { .. }
                    | OperationKind::ReleaseNativeCall { .. }
            ) && operation.result.is_some()
            {
                diagnostics.push(ir_error(
                    operation.range,
                    "native ledger control operations cannot produce Rinzu values",
                ));
            }
            let needs_unwind = operation_failure_target(&operation.kind).is_some()
                || matches!(operation.kind, OperationKind::CancellationPoll { .. });
            match (&operation.unwind, needs_unwind) {
                (Some(plan), true)
                    if plan.release_scopes
                        == active_scopes.iter().rev().copied().collect::<Vec<_>>()
                        && plan.finally
                            == operation_cleanup_chain(&operation.kind).unwrap_or_default()
                        && plan.release_native_calls
                            == active_native_calls
                                .iter()
                                .rev()
                                .copied()
                                .collect::<Vec<_>>() => {}
                (None, false) => {}
                _ => diagnostics.push(ir_error(
                    operation.range,
                    "failure/cancellation lacks exact finally-before-scope-release plan",
                )),
            }
            if operation_failure_target(&operation.kind)
                .is_some_and(|target| target != function.failure)
            {
                diagnostics.push(ir_error(
                    operation.range,
                    "managed failure must bypass operation Error catch entries",
                ));
            }
            if let Some(error) = operation_error_edge(&operation.kind) {
                let target_scopes = function
                    .blocks
                    .iter()
                    .find(|block| block.id == error.target)
                    .map(|block| block.entry_scopes.as_slice())
                    .unwrap_or_default();
                if error.release_native_calls
                    != active_native_calls
                        .iter()
                        .rev()
                        .copied()
                        .collect::<Vec<_>>()
                    || !active_scopes.starts_with(target_scopes)
                    || error.release_scopes
                        != active_scopes[target_scopes.len().min(active_scopes.len())..]
                            .iter()
                            .rev()
                            .copied()
                            .collect::<Vec<_>>()
                    || !operation_cleanup_chain(&operation.kind)
                        .unwrap_or_default()
                        .starts_with(&error.cleanup_chain)
                {
                    diagnostics.push(ir_error(
                        operation.range,
                        "operation Error edge has an invalid scope/finally release plan",
                    ));
                }
                verify_target(error.target, &block_ids, operation.range, diagnostics);
                verify_chain(
                    &error.cleanup_chain,
                    &block_ids,
                    operation.range,
                    diagnostics,
                );
                if error.target != function.failure
                    && !function.blocks.iter().any(|block| {
                        block.id == error.target
                            && block.operations.first().is_some_and(|op| {
                                matches!(
                                    op.kind,
                                    OperationKind::CatchError {
                                        observes_failure: true
                                    } | OperationKind::CatchErrors {
                                        observes_failure: true
                                    } | OperationKind::RecordTaskError { .. }
                                )
                            })
                    })
                {
                    diagnostics.push(ir_error(
                        operation.range,
                        "operation Error edge does not target a catch entry",
                    ));
                }
            }
            errors::verify_operation(module, operation, index, &value_types, diagnostics);
            match &operation.kind {
                OperationKind::Parameter { index } => {
                    if !operation.result.as_ref().is_some_and(|result| {
                        function.parameter_types.get(*index as usize) == Some(&result.ty)
                    }) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "parameter index/result type does not match the function signature",
                        ));
                    }
                }
                OperationKind::ManagedCall {
                    target,
                    arguments,
                    failure,
                    cleanup_chain,
                    ..
                } => {
                    let valid_signature =
                        reference_signature(module, target).is_some_and(|signature| {
                            arguments.len() == signature.parameters.len()
                                && arguments
                                    .iter()
                                    .zip(&signature.parameters)
                                    .all(|(value, ty)| value_types.get(value) == Some(ty))
                                && operation
                                    .result
                                    .as_ref()
                                    .map_or(*signature.result == Type::Unit, |result| {
                                        result.ty == *signature.result
                                    })
                        });
                    if !valid_signature {
                        diagnostics.push(ir_error(
                            operation.range,
                            "managed call arguments/result do not match the target signature",
                        ));
                    }
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if !has_poll_before(block, index, PollReason::ManagedCall) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "managed call lacks an immediately preceding cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::Spawn {
                    failure,
                    admission,
                    settle_domain,
                    cleanup_chain,
                    ..
                } => {
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if *admission != SpawnAdmission::Required
                        || *settle_domain != ResumeDomain::Origin
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "spawn lacks admission or origin-Domain settlement",
                        ));
                    }
                    if !has_poll_before(block, index, PollReason::Spawn) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "spawn lacks an immediately preceding cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::PrepareNativeCall {
                    call,
                    signature,
                    arguments,
                    retained,
                    callbacks,
                    ..
                } => {
                    if signature.is_async
                        || signature.is_supplant
                        || (callbacks.iter().any(|c| c.capture.is_some())
                            && !function.facts.requires_owner)
                        || arguments != retained
                        || arguments.len() != signature.parameters.len()
                        || !module
                            .source_native_signatures
                            .iter()
                            .any(|declared| declared == signature)
                        || signature.parameters.iter().zip(arguments).enumerate().any(
                            |(index, (ty, argument))| {
                                let expected = callbacks
                                    .iter()
                                    .find(|c| c.index == index)
                                    .and_then(|c| c.capture.as_ref())
                                    .cloned()
                                    .unwrap_or_else(|| crate::sema::source_abi_type(ty));
                                value_types.get(argument) != Some(&expected)
                            },
                        )
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native call preparation lacks exact retained arguments or signature",
                        ));
                    }
                    let expected: Vec<_> = signature
                        .parameters
                        .iter()
                        .enumerate()
                        .filter_map(|(index, ty)| {
                            matches!(ty, crate::SourceAbiType::Callback { .. }).then_some(index)
                        })
                        .collect();
                    if callbacks
                        .iter()
                        .map(|callback| callback.index)
                        .collect::<Vec<_>>()
                        != expected
                        || callbacks.iter().any(|callback| {
                            let Some(ty) = signature.parameters.get(callback.index) else {
                                return true;
                            };
                            let Type::FunctionReference(expected) =
                                crate::sema::source_abi_type(ty)
                            else {
                                return true;
                            };
                            !callback_plan_valid(module, callback, &expected)
                        })
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native callback preparation loses a target or its ABI contract",
                        ));
                    }
                    if native_calls
                        .insert(*call, (signature.clone(), arguments.clone(), block.id, 0))
                        .is_some()
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native call ledger identity was reused",
                        ));
                    }
                    active_native_calls.push(*call);
                    if !has_poll_before(block, index, PollReason::NativeCall) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native call preparation lacks cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::CloseNativeCall { call } => match native_calls.get_mut(call) {
                    Some((_, _, owner, state)) if *owner == block.id && *state == 1 => *state = 2,
                    _ => diagnostics.push(ir_error(
                        operation.range,
                        "native admission close lacks unique physical return",
                    )),
                },
                OperationKind::SettleNativeAdmissions { call, origin } => {
                    if *origin != ResumeDomain::Origin {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native admissions must settle on their origin",
                        ));
                    }
                    match native_calls.get_mut(call) {
                        Some((_, _, owner, state)) if *owner == block.id && *state == 2 => {
                            *state = 3
                        }
                        _ => diagnostics.push(ir_error(
                            operation.range,
                            "native admission settlement precedes gate close",
                        )),
                    }
                }
                OperationKind::ReadNativeResult { call, .. } => match native_calls.get_mut(call) {
                    Some((signature, _, owner, state)) if *owner == block.id && *state == 3 => {
                        if operation.result.as_ref().map(|value| &value.ty)
                            != Some(&crate::sema::source_abi_type(&signature.result))
                        {
                            diagnostics.push(ir_error(
                                operation.range,
                                "native result does not match its prepared ABI",
                            ));
                        }
                        *state = 4;
                    }
                    _ => diagnostics.push(ir_error(
                        operation.range,
                        "native result acquisition precedes admission settlement",
                    )),
                },
                OperationKind::ReleaseNativeCall { call } => {
                    if active_native_calls.pop() != Some(*call) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native root release is not balanced",
                        ));
                    }
                    match native_calls.get_mut(call) {
                        Some((_, _, owner, state)) if *owner == block.id && *state == 4 => {
                            *state = 5
                        }
                        _ => diagnostics.push(ir_error(
                            operation.range,
                            "native root release lacks settled typed result",
                        )),
                    }
                }
                OperationKind::EnterScope { scope } => {
                    if scope_info
                        .get(scope)
                        .is_none_or(|info| info.parent != active_scopes.last().copied())
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "scope entry has no matching lexical parent",
                        ));
                    }
                    active_scopes.push(*scope);
                }
                OperationKind::ExitScope {
                    scope,
                    deferred_finally,
                } => {
                    if active_scopes.pop() != Some(*scope)
                        || scope_info
                            .get(scope)
                            .is_none_or(|info| &info.deferred_finally != deferred_finally)
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "scope exit is unbalanced or loses pending finally",
                        ));
                    }
                }
                OperationKind::CatchError { observes_failure } => {
                    if index != 0
                        || !observes_failure
                        || operation.result.as_ref().map(|v| &v.ty) != Some(&Type::Error)
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "catch entry must observe a recoverable Error exactly once",
                        ));
                    }
                }
                OperationKind::FunctionReference { target } => {
                    if operation.result.as_ref().map(|value| &value.ty)
                        != reference_signature(module, target)
                            .map(|signature| Type::FunctionReference(Box::new(signature)))
                            .as_ref()
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "function reference target or signature is invalid",
                        ));
                    }
                }
                OperationKind::Constant(ConstantValue::String(_) | ConstantValue::Bytes(_)) => {
                    if !has_poll_before(block, index, PollReason::Allocation) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "managed literal lacks an allocation poll",
                        ));
                    }
                }
                OperationKind::FormatString {
                    parts,
                    error,
                    failure,
                    cleanup_chain,
                } => {
                    let native_display = parts.iter().any(|(_, ty)| {
                        crate::sema::display_reads_native_record(&module.source_types, ty)
                    });
                    if error.is_some() != native_display
                        || failure.is_some() != native_display
                        || !native_display && !cleanup_chain.is_empty()
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native record formatting requires a Busy Error edge",
                        ));
                    }
                    if parts.iter().any(|(value, ty)| {
                        value_types.get(value) != Some(ty) || matches!(ty, Type::Never)
                    }) || operation.result.as_ref().map(|value| &value.ty) != Some(&Type::String)
                        || !has_poll_before(block, index, PollReason::Allocation)
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "formatted String requires typed parts, String result and allocation poll",
                        ));
                    }
                }
                OperationKind::ErrorMessage { error } => {
                    if value_types.get(error) != Some(&Type::Error)
                        || operation.result.as_ref().map(|value| &value.ty) != Some(&Type::String)
                        || !has_poll_before(block, index, PollReason::Allocation)
                    {
                        diagnostics.push(ir_error(operation.range, "Error message projection requires an Error operand, String result and GC poll"));
                    }
                }
                OperationKind::Stdout {
                    operand,
                    ty,
                    failure,
                    cleanup_chain,
                    ..
                } => {
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if value_types.get(operand) != Some(ty)
                        || matches!(ty, Type::Never)
                        || operation.result.as_ref().map(|value| &value.ty) != Some(&Type::Unit)
                        || !has_poll_before(block, index, PollReason::NativeCall)
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "stdout requires a typed value, Unit result and native poll",
                        ));
                    }
                }
                OperationKind::TaskControl { kind, future } => {
                    let valid = if *kind == crate::TaskControlKind::Shutdown {
                        future.is_none()
                    } else if *kind == crate::TaskControlKind::ExitStatus {
                        future.is_some_and(|id| value_types.get(&id) == Some(&Type::Int))
                    } else {
                        future
                            .is_some_and(|id| matches!(value_types.get(&id), Some(Type::Future(_))))
                    };
                    if !valid
                        || !function.facts.requires_owner
                        || operation.result.as_ref().map(|v| &v.ty) != Some(&Type::Bool)
                        || !has_poll_before(block, index, PollReason::ManagedCall)
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "task control requires typed operand/Bool/Main owner and poll",
                        ));
                    }
                }
                OperationKind::DataOperation {
                    kind,
                    arguments,
                    argument_types,
                    ..
                } => {
                    let actual: Option<Vec<_>> = arguments
                        .iter()
                        .map(|id| value_types.get(id).cloned())
                        .collect();
                    let signature = actual
                        .as_ref()
                        .and_then(|types| types.first())
                        .and_then(|input| crate::core::data_signature(*kind, input));
                    let valid = if *kind == crate::DataOperation::ProcessArguments {
                        arguments.is_empty()
                            && function.facts.requires_owner
                            && matches!(operation.result.as_ref().map(|v| &v.ty),
                                Some(Type::Named(name))
                                if matches!(name.as_str(), "ArgumentSnapshot" | "rinzu.process::ArgumentSnapshot"))
                    } else if kind.is_json() {
                        operation.result.as_ref().is_some_and(|value| {
                            crate::json::signature(*kind, argument_types, &value.ty)
                        })
                    } else {
                        signature.is_some_and(|(parameters, result)| {
                            actual.as_ref() == Some(&parameters)
                                && operation.result.as_ref().map(|v| &v.ty) == Some(&result)
                        })
                    };
                    if actual.as_ref() != Some(argument_types)
                        || !valid
                        || (*kind != crate::DataOperation::CollectionFinish
                            && !has_poll_before(block, index, PollReason::Allocation))
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "data intrinsic signature or allocation poll is invalid",
                        ));
                    }
                }
                OperationKind::CoreLength { operand, kind } => {
                    let valid_argument = match kind {
                        crate::CoreLengthKind::Utf8 => {
                            value_types.get(operand) == Some(&Type::String)
                        }
                        crate::CoreLengthKind::Bytes => {
                            matches!(
                                value_types.get(operand),
                                Some(Type::FixedBytes(_) | Type::Bytes)
                            )
                        }
                    };
                    if !valid_argument
                        || operation.result.as_ref().map(|value| &value.ty)
                            != Some(&Type::Scalar(crate::AbiScalar::U64))
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "core length argument or CSize result is invalid",
                        ));
                    }
                }
                OperationKind::ListElement { operand, index } => {
                    let element = match value_types.get(operand) {
                        Some(Type::List(element)) => Some(element.as_ref()),
                        _ => None,
                    };
                    if element.is_none()
                        || value_types.get(index) != Some(&Type::Int)
                        || operation.result.as_ref().map(|v| &v.ty) != element
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "List projection has an invalid collection, index or element type",
                        ));
                    }
                }
                OperationKind::StoreField {
                    operand,
                    identity,
                    field,
                    value,
                } => {
                    let expected = module
                        .source_types
                        .get(identity)
                        .filter(|s| s.kind == "state" && !s.const_fields.contains(field))
                        .and_then(|s| s.fields.get(*field as usize))
                        .map(|(_, ty)| ty);
                    if expected.is_none()
                        || value_types.get(operand) != Some(&Type::Named(identity.clone()))
                        || value_types.get(value) != expected
                        || operation.result.is_some()
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "state update has an invalid identity, field, slot type or result",
                        ));
                    }
                }
                OperationKind::Field {
                    operand,
                    identity,
                    field,
                    error,
                    failure,
                    cleanup_chain,
                } => {
                    let expected = module
                        .source_types
                        .get(identity)
                        .filter(|schema| {
                            matches!(
                                schema.kind.as_str(),
                                "record" | "native-record" | "state" | "error"
                            )
                        })
                        .and_then(|schema| schema.fields.get(*field as usize))
                        .map(|(_, ty)| ty);
                    if expected.is_none()
                        || error.is_some() != failure.is_some()
                        || error.is_none() && !cleanup_chain.is_empty()
                        || error.is_some()
                            != module
                                .source_types
                                .get(identity)
                                .is_some_and(|schema| schema.kind == "native-record")
                        || value_types.get(operand) != Some(&Type::Named(identity.clone()))
                        || operation.result.as_ref().map(|result| &result.ty) != expected
                    {
                        diagnostics.push(ir_error(operation.range, "record projection has an invalid nominal identity, field index or result type"));
                    }
                }
                OperationKind::EnumPayload {
                    operand,
                    identity,
                    variant,
                    field,
                } => {
                    let expected = module
                        .source_types
                        .get(identity)
                        .filter(|schema| schema.kind == "enum")
                        .and_then(|schema| schema.variants.get(*variant as usize))
                        .and_then(|(_, fields)| fields.get(*field as usize))
                        .map(|(_, ty)| ty);
                    if expected.is_none()
                        || value_types.get(operand) != Some(&Type::Named(identity.clone()))
                        || operation.result.as_ref().map(|result| &result.ty) != expected
                        || !payload_is_guarded(function, block, *operand, identity, *variant)
                    {
                        diagnostics.push(ir_error(operation.range, "enum payload requires its matching switch case, nominal identity, field index and result type"));
                    }
                }
                OperationKind::ReserveSupplant {
                    reservation,
                    callee,
                    arguments,
                    retained,
                    signature,
                    targets,
                    callbacks,
                } => {
                    if callbacks.iter().map(|c| c.index).collect::<Vec<_>>() != signature.parameters.iter().enumerate().filter_map(|(i, ty)| matches!(ty, Type::FunctionReference(_)).then_some(i)).collect::<Vec<_>>() || callbacks.iter().any(|c| {
                        !matches!(signature.parameters.get(c.index), Some(Type::FunctionReference(expected)) if callback_plan_valid(module,c,expected))
                    }) { diagnostics.push(ir_error(operation.range, "supplant callback plan loses its capture or ABI")); }
                    if targets.iter().any(|target| {
                        reference_signature(module, target).as_ref() != Some(signature)
                    }) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "supplant target set disagrees with its signature",
                        ));
                    }
                    if !function.is_async || !function.facts.requires_owner || !signature.supplant {
                        diagnostics.push(ir_error(operation.range, "supplant reservation requires an async owner function and supplant signature"));
                    }
                    let mut expected_retained = arguments.clone();
                    expected_retained.extend(callee);
                    if retained != &expected_retained
                        || arguments.len() != signature.parameters.len()
                        || signature.parameters.iter().zip(arguments).enumerate().any(
                            |(index, (ty, argument))| {
                                value_types.get(argument)
                                    != Some(
                                        callbacks
                                            .iter()
                                            .find(|c| c.index == index)
                                            .and_then(|c| c.capture.as_ref())
                                            .unwrap_or(ty),
                                    )
                            },
                        )
                        || callee.is_some_and(|value| {
                            value_types.get(&value)
                                != Some(&Type::FunctionReference(Box::new(signature.clone())))
                        })
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "supplant reservation lacks exact argument types or retained values",
                        ));
                    }
                    if reservations
                        .insert(*reservation, ((*signature.result).clone(), block.id, 0))
                        .is_some()
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "supplant reservation identity was reused",
                        ));
                    }
                    if !has_poll_before(block, index, PollReason::Await) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "supplant reservation lacks cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::StartSupplant {
                    reservation,
                    entry_domain,
                    suspend_before_entry,
                } => {
                    if *entry_domain != NativeEntryDomain::ProcessMain || !suspend_before_entry {
                        diagnostics.push(ir_error(
                            operation.range,
                            "supplant entry requires process-main dispatch after suspension",
                        ));
                    }
                    match reservations.get_mut(reservation) {
                        Some((_, owner_block, state))
                            if *owner_block == block.id && *state == 0 =>
                        {
                            *state = 1
                        }
                        _ => diagnostics.push(ir_error(
                            operation.range,
                            "supplant entry lacks a unique preceding reservation",
                        )),
                    }
                }
                OperationKind::SettleSupplant {
                    reservation,
                    release,
                    failure,
                    cleanup_chain,
                    error: _,
                } => {
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    match reservations.get_mut(reservation) {
                        Some((ty, owner_block, state))
                            if *owner_block == block.id && *state == 1 =>
                        {
                            if !release
                                || operation.result.as_ref().map(|value| &value.ty) != Some(ty)
                            {
                                diagnostics.push(ir_error(
                                    operation.range,
                                    "supplant settlement lacks release or typed result",
                                ));
                            }
                            *state = 2;
                        }
                        _ => diagnostics.push(ir_error(
                            operation.range,
                            "supplant settlement lacks a unique preceding entry",
                        )),
                    }
                }
                OperationKind::IndirectCall {
                    callee,
                    arguments,
                    failure,
                    cleanup_chain,
                    targets,
                    error: _,
                } => {
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    let valid = match value_types.get(callee) {
                        Some(Type::FunctionReference(signature)) => {
                            !signature.supplant
                                && targets.iter().collect::<HashSet<_>>().len() == targets.len()
                                && targets.iter().all(|target| {
                                    reference_signature(module, target).as_ref()
                                        == Some(signature.as_ref())
                                })
                                && signature.parameters.len() == arguments.len()
                                && signature
                                    .parameters
                                    .iter()
                                    .zip(arguments)
                                    .all(|(ty, argument)| value_types.get(argument) == Some(ty))
                                && operation.result.as_ref().map(|value| &value.ty)
                                    == Some(signature.result.as_ref())
                        }
                        _ => false,
                    };
                    if !valid {
                        diagnostics.push(ir_error(
                            operation.range,
                            "indirect call does not match its function reference type",
                        ));
                    }
                    if !has_poll_before(block, index, PollReason::ManagedCall) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "indirect call lacks cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::SourceNativeCall {
                    call,
                    signature,
                    arguments,
                    ..
                } => {
                    if !module
                        .source_native_signatures
                        .iter()
                        .any(|declared| declared == signature)
                        || signature.is_supplant
                        || signature.parameters.len() != arguments.len()
                        || signature
                            .parameters
                            .iter()
                            .zip(arguments)
                            .any(|(ty, value)| {
                                call.is_none()
                                    && value_types.get(value)
                                        != Some(&crate::sema::source_abi_type(ty))
                            })
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "source native call disagrees with the module ABI interface",
                        ));
                    }
                    if let Some(call) = call {
                        match native_calls.get_mut(call) {
                            Some((prepared, prepared_arguments, owner, state))
                                if prepared == signature
                                    && prepared_arguments == arguments
                                    && *owner == block.id
                                    && *state == 0
                                    && !signature.is_async
                                    && operation.result.is_none() =>
                            {
                                *state = 1
                            }
                            _ => diagnostics.push(ir_error(
                                operation.range,
                                "physical native entry lacks a complete unique preparation",
                            )),
                        }
                    } else {
                        let expected =
                            Type::Future(Box::new(crate::sema::source_abi_type(&signature.result)));
                        if !signature.is_async
                            || operation.result.as_ref().map(|value| &value.ty) != Some(&expected)
                            || !has_poll_before(block, index, PollReason::NativeCall)
                        {
                            diagnostics.push(ir_error(
                                operation.range,
                                "service call lacks its async signature or poll",
                            ));
                        }
                    }
                }
                OperationKind::NativeCall {
                    failure,
                    transition,
                    cleanup_chain,
                    ..
                } => {
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if !matches!(
                        transition,
                        NativeTransition::NativeSafe | NativeTransition::OwnerThreadSync
                    ) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native call has no verified transition",
                        ));
                    }
                    if !has_poll_before(block, index, PollReason::NativeCall) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "native call lacks an immediately preceding cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::Await {
                    future,
                    consume,
                    failure,
                    ready,
                    pending,
                    resume_domain,
                    cleanup_chain,
                    ..
                } => {
                    if *consume && !consumable_future(module, function, *future) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "consuming await lacks a fresh, single-use Future proof",
                        ));
                    }
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if !function.is_async {
                        diagnostics.push(ir_error(
                            operation.range,
                            "await appears in a synchronous IR function",
                        ));
                    }
                    if *resume_domain != ResumeDomain::Origin {
                        diagnostics.push(ir_error(
                            operation.range,
                            "await continuation does not resume on its origin Domain",
                        ));
                    }
                    if *ready != AwaitReadyPath::ContinueInline
                        || *pending != AwaitPendingPath::SuspendAndResume
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "await does not encode both ready and pending paths",
                        ));
                    }
                    if !has_poll_before(block, index, PollReason::Await) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "await lacks an immediately preceding cancellation/GC poll",
                        ));
                    }
                }
                OperationKind::ErrorGroupOutcome {
                    cleanup,
                    failure,
                    cleanup_chain,
                    ..
                }
                | OperationKind::TaskScopeOutcome {
                    cleanup,
                    failure,
                    cleanup_chain,
                    ..
                } => {
                    verify_target(*cleanup, &block_ids, operation.range, diagnostics);
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                }
                OperationKind::EnterTaskScope { .. } | OperationKind::JoinTaskScope { .. } => {
                    if !function.is_async || operation.result.is_some() {
                        diagnostics.push(ir_error(
                            operation.range,
                            "task group operation requires an async function and no result",
                        ));
                    }
                }
                OperationKind::EnterTry { cleanup } | OperationKind::LeaveTry { cleanup } => {
                    verify_target(*cleanup, &block_ids, operation.range, diagnostics);
                }
                OperationKind::CancellationPoll { cleanup_chain } => {
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                }
                OperationKind::GcSafepoint { stack_map, .. } => {
                    verify_stack_map(
                        stack_map,
                        &local_values,
                        &value_types,
                        operation.range,
                        diagnostics,
                    );
                }
                OperationKind::Unary {
                    failure,
                    cleanup_chain,
                    ..
                }
                | OperationKind::Binary {
                    failure,
                    cleanup_chain,
                    ..
                } => {
                    if let Some(failure) = failure {
                        verify_target(*failure, &block_ids, operation.range, diagnostics);
                        verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    }
                }
                OperationKind::BytesPad { operand, from, to } => {
                    if from >= to
                        || value_types.get(operand) != Some(&Type::FixedBytes(*from))
                        || !operation
                            .result
                            .as_ref()
                            .is_some_and(|result| result.ty == Type::FixedBytes(*to))
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "Bytes pad has invalid source/result lengths",
                        ));
                    }
                    if !has_poll_before(block, index, PollReason::Allocation) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "Bytes pad lacks an allocation poll",
                        ));
                    }
                }
                OperationKind::NumericConvert {
                    operand,
                    source,
                    failure,
                    cleanup_chain,
                    error: _,
                } => {
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if !source.is_numeric()
                        || value_types.get(operand) != Some(source)
                        || !operation
                            .result
                            .as_ref()
                            .is_some_and(|result| result.ty.is_numeric() && result.ty != *source)
                    {
                        diagnostics.push(ir_error(
                            operation.range,
                            "numeric conversion has invalid source/result types",
                        ));
                    }
                }
                OperationKind::Construct {
                    failure,
                    cleanup_chain,
                    ..
                } => {
                    if !construct_is_typed(module, operation, &value_types) {
                        diagnostics.push(ir_error(operation.range, "constructor arguments or variant do not match the declared result layout"));
                    }
                    verify_target(*failure, &block_ids, operation.range, diagnostics);
                    verify_chain(cleanup_chain, &block_ids, operation.range, diagnostics);
                    if !has_poll_before(block, index, PollReason::Allocation) {
                        diagnostics.push(ir_error(
                            operation.range,
                            "allocation lacks an immediately preceding cancellation/GC poll",
                        ));
                    }
                }
                _ => {}
            }
        }
        if !active_native_calls.is_empty() {
            diagnostics.push(ir_error(
                block.range,
                "native call ledger escapes its basic block",
            ));
        }
        if let Terminator::Switch {
            value,
            identity,
            cases,
        } = &block.terminator
        {
            let schema = module
                .source_types
                .get(identity)
                .filter(|schema| schema.kind == "enum");
            let tags = cases.iter().map(|(tag, _)| *tag).collect::<HashSet<_>>();
            if value_types.get(value) != Some(&Type::Named(identity.clone()))
                || !schema.is_some_and(|schema| {
                    cases.len() == schema.variants.len()
                        && tags.len() == cases.len()
                        && tags
                            .iter()
                            .all(|tag| (*tag as usize) < schema.variants.len())
                })
            {
                diagnostics.push(ir_error(
                    block.range,
                    "switch must cover each declared variant of its nominal operand exactly once",
                ));
            }
        }
        verify_terminator(
            function,
            block,
            &active_scopes,
            &block_ids,
            &local_values,
            &value_types,
            diagnostics,
        );
    }
    let has_async_start = function
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .any(|operation| matches!(operation.kind, OperationKind::AsyncStart));
    if has_async_start != function.is_async {
        diagnostics.push(ir_error(
            function.range,
            "async start marker does not match the function kind",
        ));
    }
    let has_entry_poll = function.blocks.iter().any(|block| {
        block.operations.windows(2).any(|operations| {
            matches!(operations[0].kind, OperationKind::CancellationPoll { .. })
                && matches!(
                    operations[1].kind,
                    OperationKind::GcSafepoint {
                        reason: PollReason::FunctionEntry,
                        ..
                    }
                )
        })
    });
    if !has_entry_poll {
        diagnostics.push(ir_error(
            function.range,
            "function lacks an entry cancellation/GC poll",
        ));
    }
    if native_calls.values().any(|(_, _, _, state)| *state != 5) {
        diagnostics.push(ir_error(
            function.range,
            "native call roots are not released exactly once",
        ));
    }
    if reservations.values().any(|(_, _, state)| *state != 2) {
        diagnostics.push(ir_error(
            function.range,
            "supplant reservation is not settled exactly once",
        ));
    }
}

fn construct_is_typed(
    module: &IrModule,
    operation: &Operation,
    types: &HashMap<ValueId, Type>,
) -> bool {
    let OperationKind::Construct {
        target,
        variant,
        arguments,
        ..
    } = &operation.kind
    else {
        return false;
    };
    let Some(result) = &operation.result else {
        return false;
    };
    match &result.ty {
        Type::List(element) => {
            target == "List"
                && variant.is_none()
                && arguments
                    .iter()
                    .all(|value| types.get(value) == Some(element.as_ref()))
        }
        Type::Map(key, value) => {
            target == "Map"
                && variant.is_none()
                && arguments.len().is_multiple_of(2)
                && arguments.chunks_exact(2).all(|pair| {
                    types.get(&pair[0]) == Some(key.as_ref())
                        && types.get(&pair[1]) == Some(value.as_ref())
                })
        }
        Type::Named(identity) if identity == target => {
            module.source_types.get(identity).is_some_and(|schema| {
                let fields = match (schema.kind.as_str(), variant) {
                    ("record" | "native-record" | "state" | "error", None) => Some(&schema.fields),
                    ("enum", Some(variant)) => schema
                        .variants
                        .get(*variant as usize)
                        .map(|(_, fields)| fields),
                    _ => None,
                };
                fields.is_some_and(|fields| {
                    arguments.len() == fields.len()
                        && fields
                            .iter()
                            .zip(arguments)
                            .all(|((_, ty), value)| types.get(value) == Some(ty))
                })
            })
        }
        _ => false,
    }
}

fn payload_is_guarded(
    function: &IrFunction,
    block: &BasicBlock,
    operand: ValueId,
    identity: &str,
    variant: u32,
) -> bool {
    let Some((load_index, name)) =
        block
            .operations
            .iter()
            .enumerate()
            .find_map(|(index, operation)| {
                if operation
                    .result
                    .as_ref()
                    .is_some_and(|result| result.id == operand)
                    && let OperationKind::LoadLocal { name } = &operation.kind
                {
                    Some((index, name))
                } else {
                    None
                }
            })
    else {
        return false;
    };
    if block.operations[..load_index].iter().any(|operation| matches!(&operation.kind, OperationKind::StoreLocal { name: written, .. } if written == name)) {
        return false;
    }
    let mut guarded = false;
    for predecessor in &function.blocks {
        match &predecessor.terminator {
            Terminator::Switch {
                value,
                identity: owner,
                cases,
            } if cases.iter().any(|(_, target)| *target == block.id) => {
                let saved_value = predecessor.operations.iter().rev().find_map(|operation| {
                    if let OperationKind::StoreLocal { name: saved, value } = &operation.kind
                        && saved == name
                    {
                        Some(value)
                    } else {
                        None
                    }
                });
                if owner != identity
                    || cases
                        .iter()
                        .any(|(tag, target)| *target == block.id && *tag != variant)
                    || saved_value != Some(value)
                {
                    return false;
                }
                guarded = true;
            }
            Terminator::Goto { target, .. } if *target == block.id => return false,
            Terminator::Branch {
                then_target,
                else_target,
                ..
            } if *then_target == block.id || *else_target == block.id => return false,
            _ => {}
        }
    }
    guarded
}

fn verify_terminator(
    function: &IrFunction,
    block: &BasicBlock,
    active_scopes: &[SourceRange],
    block_ids: &HashSet<BlockId>,
    local_values: &HashSet<ValueId>,
    value_types: &HashMap<ValueId, Type>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match &block.terminator {
        Terminator::Pending => {
            diagnostics.push(ir_error(block.range, "basic block has no terminator"))
        }
        Terminator::Goto {
            target,
            cleanup_chain,
            reason,
            release_scopes,
        } => {
            let target_scopes = function
                .blocks
                .iter()
                .find(|block| block.id == *target)
                .map(|block| block.entry_scopes.as_slice())
                .unwrap_or_default();
            if !active_scopes.starts_with(target_scopes)
                || release_scopes
                    != &active_scopes[target_scopes.len().min(active_scopes.len())..]
                        .iter()
                        .rev()
                        .copied()
                        .collect::<Vec<_>>()
            {
                diagnostics.push(ir_error(
                    block.range,
                    "control-flow edge lacks exact lexical scope release",
                ));
            }
            verify_target(*target, block_ids, block.range, diagnostics);
            verify_chain(cleanup_chain, block_ids, block.range, diagnostics);
            if target.0 <= block.id.0 && matches!(reason, ExitReason::Continue) {
                let has_poll = block.operations.iter().any(|operation| {
                    matches!(operation.kind, OperationKind::CancellationPoll { .. })
                });
                let has_safepoint = block.operations.iter().any(|operation| {
                    matches!(
                        operation.kind,
                        OperationKind::GcSafepoint {
                            reason: PollReason::LoopBackedge,
                            ..
                        }
                    )
                });
                if !has_poll || !has_safepoint {
                    diagnostics.push(ir_error(
                        block.range,
                        "loop backedge lacks cancellation poll or GC safepoint",
                    ));
                }
            }
        }
        Terminator::Branch {
            condition,
            then_target,
            else_target,
        } => {
            if !local_values.contains(condition) {
                diagnostics.push(ir_error(
                    block.range,
                    "branch condition is not defined in its block",
                ));
            }
            if value_types.get(condition) != Some(&Type::Bool) {
                diagnostics.push(ir_error(
                    block.range,
                    "branch condition must have type Bool",
                ));
            }
            verify_target(*then_target, block_ids, block.range, diagnostics);
            verify_target(*else_target, block_ids, block.range, diagnostics);
            for target in [then_target, else_target] {
                if function
                    .blocks
                    .iter()
                    .find(|b| b.id == *target)
                    .is_some_and(|b| b.entry_scopes != active_scopes)
                {
                    diagnostics.push(ir_error(
                        block.range,
                        "branch changes lexical scopes without a release edge",
                    ));
                }
            }
        }
        Terminator::Switch { value, cases, .. } => {
            if !local_values.contains(value) {
                diagnostics.push(ir_error(
                    block.range,
                    "switch value is not defined in its block",
                ));
            }
            for (_, target) in cases {
                verify_target(*target, block_ids, block.range, diagnostics);
                if function
                    .blocks
                    .iter()
                    .find(|block| block.id == *target)
                    .is_some_and(|block| block.entry_scopes != active_scopes)
                {
                    diagnostics.push(ir_error(
                        block.range,
                        "switch changes lexical scopes without a release edge",
                    ));
                }
            }
        }
        Terminator::Return {
            value,
            cleanup_chain,
            release_scopes,
        } => {
            let actual = value
                .as_ref()
                .map_or(Some(&Type::Unit), |value| value_types.get(value));
            if actual != Some(&function.result_type) && actual != Some(&Type::Never) {
                diagnostics.push(ir_error(
                    block.range,
                    "return type does not match the function result",
                ));
            }
            if release_scopes != &active_scopes.iter().rev().copied().collect::<Vec<_>>() {
                diagnostics.push(ir_error(
                    block.range,
                    "return lacks complete lexical scope release after finally",
                ));
            }
            if value.is_some_and(|id| !local_values.contains(&id)) {
                diagnostics.push(ir_error(
                    block.range,
                    "return value is not defined in its block",
                ));
            }
            verify_chain(cleanup_chain, block_ids, block.range, diagnostics);
        }
        Terminator::Fail | Terminator::Unreachable => {}
    }
}

fn has_poll_before(block: &BasicBlock, index: usize, reason: PollReason) -> bool {
    index >= 2
        && matches!(
            block.operations[index - 2].kind,
            OperationKind::CancellationPoll { .. }
        )
        && matches!(
            block.operations[index - 1].kind,
            OperationKind::GcSafepoint {
                reason: actual,
                ..
            } if actual == reason
        )
}

fn verify_stack_map(
    stack_map: &StackMap,
    local_values: &HashSet<ValueId>,
    value_types: &HashMap<ValueId, Type>,
    range: SourceRange,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen_values = HashSet::new();
    let mut seen_locals = HashSet::new();
    for root in &stack_map.roots {
        match root {
            StackRoot::Value { value, ty } => {
                if !seen_values.insert(*value) {
                    diagnostics.push(ir_error(range, format!("duplicate stack root {value:?}")));
                }
                if !local_values.contains(value) || value_types.get(value) != Some(ty) {
                    diagnostics.push(ir_error(
                        range,
                        format!("stack root {value:?} has no matching dominating value"),
                    ));
                }
                if !ty.is_managed_reference() {
                    diagnostics.push(ir_error(
                        range,
                        format!("stack root {value:?} is not a managed reference"),
                    ));
                }
            }
            StackRoot::Local { name, ty } => {
                if !seen_locals.insert(name) {
                    diagnostics.push(ir_error(
                        range,
                        format!("duplicate local stack root {name}"),
                    ));
                }
                if !ty.is_managed_reference() {
                    diagnostics.push(ir_error(
                        range,
                        format!("local stack root {name} is not a managed reference"),
                    ));
                }
            }
        }
    }
    for value in local_values {
        if value_types
            .get(value)
            .is_some_and(Type::is_managed_reference)
            && !seen_values.contains(value)
        {
            diagnostics.push(ir_error(
                range,
                format!("managed value {value:?} is missing from the stack map"),
            ));
        }
    }
}

fn consumable_future(module: &IrModule, function: &IrFunction, future: ValueId) -> bool {
    let async_target = |target: &str| {
        module
            .functions
            .iter()
            .any(|f| f.name == target && f.is_async)
    };
    let operations = function.blocks.iter().flat_map(|block| &block.operations);
    let Some(producer) = operations
        .clone()
        .find(|op| op.result.as_ref().is_some_and(|v| v.id == future))
    else {
        return false;
    };
    let fresh = match &producer.kind {
        OperationKind::ManagedCall { target, .. } => async_target(target),
        OperationKind::IndirectCall { targets, .. } => {
            !targets.is_empty() && targets.iter().all(|t| async_target(t))
        }
        OperationKind::SourceNativeCall { signature, .. } => signature.is_async,
        OperationKind::Spawn { .. } => true,
        _ => false,
    };
    fresh
        && operations
            .flat_map(|op| operation_inputs(&op.kind))
            .filter(|id| *id == future)
            .count()
            == 1
        && function.blocks.iter().all(|block| match &block.terminator {
            Terminator::Return {
                value: Some(value), ..
            }
            | Terminator::Switch { value, .. } => *value != future,
            Terminator::Branch { condition, .. } => *condition != future,
            _ => true,
        })
}

fn operation_inputs(kind: &OperationKind) -> Vec<ValueId> {
    match kind {
        OperationKind::TaskControl { future, .. } => future.iter().copied().collect(),
        OperationKind::FormatString { parts, .. } => {
            parts.iter().map(|(value, _)| *value).collect()
        }
        OperationKind::StoreLocal { value, .. } => vec![*value],
        OperationKind::StoreField { operand, value, .. } => vec![*operand, *value],
        OperationKind::ListElement { operand, index } => vec![*operand, *index],
        OperationKind::ErrorMessage { error } | OperationKind::ErrorField { error, .. } => {
            vec![*error]
        }
        OperationKind::ThrowError { value, .. } => vec![*value],
        OperationKind::ErrorCast { operand } | OperationKind::ErrorIs { operand, .. } => {
            vec![*operand]
        }
        OperationKind::ErrorMatch { errors, .. }
        | OperationKind::ErrorIndex { errors, .. }
        | OperationKind::ErrorRest { errors, .. } => vec![*errors],
        OperationKind::Unary { operand, .. }
        | OperationKind::NumericConvert { operand, .. }
        | OperationKind::BytesPad { operand, .. }
        | OperationKind::CoreLength { operand, .. }
        | OperationKind::Stdout { operand, .. }
        | OperationKind::Field { operand, .. }
        | OperationKind::EnumPayload { operand, .. } => {
            vec![*operand]
        }
        OperationKind::Binary { left, right, .. } => vec![*left, *right],
        OperationKind::DataOperation { arguments, .. }
        | OperationKind::PrepareNativeCall { arguments, .. }
        | OperationKind::ManagedCall { arguments, .. }
        | OperationKind::NativeCall { arguments, .. }
        | OperationKind::SourceNativeCall { arguments, .. }
        | OperationKind::Construct { arguments, .. }
        | OperationKind::Spawn { arguments, .. } => arguments.clone(),
        OperationKind::ReserveSupplant {
            callee, arguments, ..
        } => {
            let mut inputs = arguments.clone();
            inputs.extend(callee);
            inputs
        }
        OperationKind::IndirectCall {
            callee, arguments, ..
        } => {
            let mut inputs = vec![*callee];
            inputs.extend(arguments);
            inputs
        }
        OperationKind::Await { future, .. } => vec![*future],
        _ => Vec::new(),
    }
}

fn operation_inputs_mut(kind: &mut OperationKind) -> Vec<&mut ValueId> {
    match kind {
        OperationKind::TaskControl { future, .. } => future.iter_mut().collect(),
        OperationKind::FormatString { parts, .. } => {
            parts.iter_mut().map(|(value, _)| value).collect()
        }
        OperationKind::StoreLocal { value, .. } => vec![value],
        OperationKind::StoreField { operand, value, .. } => vec![operand, value],
        OperationKind::ListElement { operand, index } => vec![operand, index],
        OperationKind::ErrorMessage { error } | OperationKind::ErrorField { error, .. } => {
            vec![error]
        }
        OperationKind::ThrowError { value, .. } => vec![value],
        OperationKind::ErrorCast { operand } | OperationKind::ErrorIs { operand, .. } => {
            vec![operand]
        }
        OperationKind::ErrorMatch { errors, .. }
        | OperationKind::ErrorIndex { errors, .. }
        | OperationKind::ErrorRest { errors, .. } => vec![errors],
        OperationKind::Unary { operand, .. }
        | OperationKind::NumericConvert { operand, .. }
        | OperationKind::BytesPad { operand, .. }
        | OperationKind::CoreLength { operand, .. }
        | OperationKind::Stdout { operand, .. }
        | OperationKind::Field { operand, .. }
        | OperationKind::EnumPayload { operand, .. } => vec![operand],
        OperationKind::Binary { left, right, .. } => vec![left, right],
        OperationKind::DataOperation { arguments, .. }
        | OperationKind::PrepareNativeCall { arguments, .. }
        | OperationKind::ManagedCall { arguments, .. }
        | OperationKind::NativeCall { arguments, .. }
        | OperationKind::SourceNativeCall { arguments, .. }
        | OperationKind::Construct { arguments, .. }
        | OperationKind::Spawn { arguments, .. } => arguments.iter_mut().collect(),
        OperationKind::ReserveSupplant {
            callee, arguments, ..
        } => {
            let mut inputs = arguments.iter_mut().collect::<Vec<_>>();
            inputs.extend(callee);
            inputs
        }
        OperationKind::IndirectCall {
            callee, arguments, ..
        } => {
            let mut inputs = vec![callee];
            inputs.extend(arguments);
            inputs
        }
        OperationKind::Await { future, .. } => vec![future],
        _ => Vec::new(),
    }
}

fn verify_target(
    target: BlockId,
    blocks: &HashSet<BlockId>,
    range: SourceRange,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !blocks.contains(&target) {
        diagnostics.push(ir_error(
            range,
            format!("edge targets missing block {:?}", target),
        ));
    }
}

fn verify_chain(
    chain: &[BlockId],
    blocks: &HashSet<BlockId>,
    range: SourceRange,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen = HashSet::new();
    for target in chain {
        verify_target(*target, blocks, range, diagnostics);
        if !seen.insert(*target) {
            diagnostics.push(ir_error(
                range,
                "cleanup chain executes a finally block more than once",
            ));
        }
    }
}

fn ir_error(range: SourceRange, message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(
        DiagnosticCode::IrVerification,
        range,
        message,
        "report this compiler bug with the source file and diagnostic code",
    )
}

fn numeric_literal(value: i128, ty: &Type) -> ConstantValue {
    match ty.scalar() {
        Some(crate::AbiScalar::F32) => ConstantValue::Float(f64::from(
            value.to_string().parse::<f32>().expect("integer spelling"),
        )),
        Some(crate::AbiScalar::F64) => {
            ConstantValue::Float(value.to_string().parse::<f64>().expect("integer spelling"))
        }
        _ => ConstantValue::Int(value as i64),
    }
}

fn reference_signature(module: &IrModule, target: &str) -> Option<crate::FunctionReferenceType> {
    if let Some(signature) = crate::core::reference_signature(target) {
        return Some(signature);
    }
    if let Some(function) = module.functions.iter().find(|f| f.name == target) {
        return Some(crate::FunctionReferenceType {
            error_bound: None,
            parameters: function.parameter_types.clone(),
            result: Box::new(if function.is_async {
                Type::Future(Box::new(function.result_type.clone()))
            } else {
                function.result_type.clone()
            }),
            supplant: false,
        });
    }
    module
        .source_native_signatures
        .iter()
        .find(|signature| signature.name == target)
        .map(|signature| crate::FunctionReferenceType {
            error_bound: None,
            parameters: signature
                .parameters
                .iter()
                .map(crate::sema::source_abi_type)
                .collect(),
            result: Box::new(if signature.is_async {
                Type::Future(Box::new(crate::sema::source_abi_type(&signature.result)))
            } else {
                crate::sema::source_abi_type(&signature.result)
            }),
            supplant: signature.is_supplant,
        })
}

fn operation_error_edge(kind: &OperationKind) -> Option<&ErrorEdge> {
    match kind {
        OperationKind::Field { error, .. } | OperationKind::FormatString { error, .. } => {
            error.as_ref()
        }
        OperationKind::ThrowError { error, .. }
        | OperationKind::CheckLocal { error, .. }
        | OperationKind::ErrorGroupOutcome { error, .. }
        | OperationKind::TaskScopeOutcome { error, .. }
        | OperationKind::PrepareNativeCall { error, .. }
        | OperationKind::Stdout { error, .. }
        | OperationKind::ReadNativeResult { error, .. }
        | OperationKind::NumericConvert { error, .. }
        | OperationKind::ManagedCall { error, .. }
        | OperationKind::IndirectCall { error, .. }
        | OperationKind::SettleSupplant { error, .. }
        | OperationKind::DataOperation { error, .. }
        | OperationKind::SourceNativeCall { error, .. }
        | OperationKind::NativeCall { error, .. }
        | OperationKind::Await { error, .. } => Some(error),
        _ => None,
    }
}

fn operation_failure_target(kind: &OperationKind) -> Option<BlockId> {
    match kind {
        OperationKind::Field { failure, .. } | OperationKind::FormatString { failure, .. } => {
            *failure
        }
        OperationKind::Unary { failure, .. } | OperationKind::Binary { failure, .. } => *failure,
        OperationKind::ThrowError { failure, .. }
        | OperationKind::CheckLocal { failure, .. }
        | OperationKind::ErrorGroupOutcome { failure, .. }
        | OperationKind::TaskScopeOutcome { failure, .. }
        | OperationKind::PrepareNativeCall { failure, .. }
        | OperationKind::Stdout { failure, .. }
        | OperationKind::ReadNativeResult { failure, .. }
        | OperationKind::NumericConvert { failure, .. }
        | OperationKind::ManagedCall { failure, .. }
        | OperationKind::IndirectCall { failure, .. }
        | OperationKind::SettleSupplant { failure, .. }
        | OperationKind::DataOperation { failure, .. }
        | OperationKind::SourceNativeCall { failure, .. }
        | OperationKind::NativeCall { failure, .. }
        | OperationKind::Await { failure, .. }
        | OperationKind::Construct { failure, .. }
        | OperationKind::Spawn { failure, .. } => Some(*failure),
        _ => None,
    }
}

fn operation_cleanup_chain(kind: &OperationKind) -> Option<&[BlockId]> {
    match kind {
        OperationKind::Field { cleanup_chain, .. }
        | OperationKind::FormatString { cleanup_chain, .. } => Some(cleanup_chain),
        OperationKind::ThrowError { cleanup_chain, .. }
        | OperationKind::CheckLocal { cleanup_chain, .. }
        | OperationKind::ErrorGroupOutcome { cleanup_chain, .. }
        | OperationKind::TaskScopeOutcome { cleanup_chain, .. }
        | OperationKind::PrepareNativeCall { cleanup_chain, .. }
        | OperationKind::Stdout { cleanup_chain, .. }
        | OperationKind::ReadNativeResult { cleanup_chain, .. }
        | OperationKind::Unary { cleanup_chain, .. }
        | OperationKind::Binary { cleanup_chain, .. }
        | OperationKind::NumericConvert { cleanup_chain, .. }
        | OperationKind::ManagedCall { cleanup_chain, .. }
        | OperationKind::IndirectCall { cleanup_chain, .. }
        | OperationKind::SettleSupplant { cleanup_chain, .. }
        | OperationKind::DataOperation { cleanup_chain, .. }
        | OperationKind::SourceNativeCall { cleanup_chain, .. }
        | OperationKind::NativeCall { cleanup_chain, .. }
        | OperationKind::Await { cleanup_chain, .. }
        | OperationKind::Construct { cleanup_chain, .. }
        | OperationKind::Spawn { cleanup_chain, .. }
        | OperationKind::CancellationPoll { cleanup_chain } => Some(cleanup_chain),
        _ => None,
    }
}

fn scope_chain_valid(scopes: &[SourceRange], info: &HashMap<SourceRange, &ScopeInfo>) -> bool {
    let mut parent = None;
    for scope in scopes {
        if info.get(scope).is_none_or(|scope| scope.parent != parent) {
            return false;
        }
        parent = Some(*scope);
    }
    true
}

fn expression_arguments(expression: &Expression) -> &Arguments {
    let ExpressionKind::Call { arguments, .. } = &expression.kind else {
        unreachable!("call expression")
    };
    arguments
}

fn callback_plan_valid(
    module: &IrModule,
    callback: &CallbackArgumentPlan,
    expected: &crate::FunctionReferenceType,
) -> bool {
    if callback.targets.iter().collect::<HashSet<_>>().len() != callback.targets.len() {
        return false;
    }
    let mut signature = expected.clone();
    if let Some(capture) = &callback.capture {
        let Type::Named(name) = capture else {
            return false;
        };
        let Some(schema) = module.source_types.get(name).filter(|s| {
            name.starts_with("$rinzu_closure_") && s.kind == "state" && s.fields.len() == 1
        }) else {
            return false;
        };
        let Some((field, Type::Named(environment))) = schema.fields.first() else {
            return false;
        };
        let Some(environment) = module
            .source_types
            .get(environment)
            .filter(|s| s.kind == "enum")
        else {
            return false;
        };
        if field != "environment"
            || environment.variants.len() != callback.targets.len()
            || callback.targets.is_empty()
            || (*signature.result != Type::Unit && signature.result.scalar().is_none())
            || callback
                .targets
                .iter()
                .enumerate()
                .any(|(i, t)| *t != format!("{name}_callback_{i}"))
        {
            return false;
        }
        signature.parameters.insert(0, capture.clone());
    }
    callback
        .targets
        .iter()
        .all(|target| reference_signature(module, target).as_ref() == Some(&signature))
}
