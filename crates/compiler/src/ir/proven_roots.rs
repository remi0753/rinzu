//! C-runtime membership facts; actual publication, marking and polls are unchanged.
use super::{
    BlockId, ConstantValue, IrFunction, IrModule, Operation, OperationKind as O, StackRoot,
    Terminator, ValueId, dead_local_roots, dead_temporary_roots, operation_error_edge,
    operation_failure_target,
};
use crate::Type;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Value(ValueId),
    Local(String),
}

// Presence proves the word is currently accepted by the managed validator.
// True additionally proves retention in the current *physical* caller frame.
// The flag follows exact copies, never merely a matching type.
type State = HashMap<Slot, bool>;
type FramePlan = HashSet<(BlockId, usize)>;
type MembershipMasks = HashMap<(BlockId, usize), u64>;

struct Function<'a> {
    ir: &'a IrFunction,
    roots: HashMap<(BlockId, usize), Vec<Slot>>,
    types: HashMap<ValueId, &'a Type>,
    reachable: Vec<BlockId>,
    predecessors: HashMap<BlockId, Vec<BlockId>>,
    forced: HashSet<BlockId>,
    edge_preserves: HashMap<BlockId, bool>,
    closed_scopes: bool,
}

impl<'a> Function<'a> {
    fn new(ir: &'a IrFunction) -> Option<Self> {
        if ir.is_async || ir.scopes.iter().any(|s| !s.deferred_finally.is_empty()) {
            return None;
        }
        for block in &ir.blocks {
            if matches!(&block.terminator,
                Terminator::Goto { cleanup_chain, .. } | Terminator::Return { cleanup_chain, .. }
                if !cleanup_chain.is_empty())
            {
                return None;
            }
            for op in &block.operations {
                // Non-normal edges terminate this activation. No handler or
                // finally may rejoin without participating in the CFG meet.
                if op.unwind.as_ref().is_some_and(|u| !u.finally.is_empty())
                    || operation_error_edge(&op.kind).is_some_and(|e| e.target != ir.failure)
                    || operation_failure_target(&op.kind).is_some_and(|b| b != ir.failure)
                    || matches!(
                        op.kind,
                        O::EnterTry { .. }
                            | O::LeaveTry { .. }
                            | O::CatchError { .. }
                            | O::CatchErrors { .. }
                            | O::ReserveSupplant { .. }
                            | O::StartSupplant { .. }
                            | O::SettleSupplant { .. }
                            | O::IndirectCall { .. }
                            | O::NativeCall { .. }
                            | O::AsyncStart
                            | O::Await { .. }
                            | O::Spawn { .. }
                            | O::EnterTaskScope { .. }
                            | O::JoinTaskScope { .. }
                            | O::TaskScopeOutcome { .. }
                            | O::EnterErrorGroup { .. }
                            | O::FinishErrorGroup { .. }
                            | O::ErrorGroupOutcome { .. }
                            | O::RecordTaskError { .. }
                            | O::TakeTaskErrors { .. }
                            | O::TaskControl { .. }
                    )
                {
                    return None;
                }
            }
        }
        Self::build(ir, HashSet::from([ir.entry]), false, true)
    }

    fn wide(ir: &'a IrFunction, closed_scopes: bool) -> Option<Self> {
        let mut forced = HashSet::from([ir.entry, ir.failure]);
        for scope in &ir.scopes {
            forced.extend(&scope.deferred_finally);
        }
        for block in &ir.blocks {
            for op in &block.operations {
                super::proven_owner::exceptional(op, &mut forced);
            }
            if let Terminator::Goto { cleanup_chain, .. }
            | Terminator::Return { cleanup_chain, .. } = &block.terminator
            {
                forced.extend(cleanup_chain);
            }
        }
        Self::build(ir, forced, true, closed_scopes)
    }

    fn build(
        ir: &'a IrFunction,
        forced: HashSet<BlockId>,
        wide: bool,
        closed_scopes: bool,
    ) -> Option<Self> {
        if forced
            .iter()
            .any(|id| !ir.blocks.iter().any(|b| b.id == *id))
        {
            return None;
        }
        let dead_values = dead_temporary_roots(ir);
        let dead_locals = dead_local_roots(ir);
        let mut roots = HashMap::new();
        let mut types = HashMap::new();
        for block in &ir.blocks {
            for (index, op) in block.operations.iter().enumerate() {
                if let Some(result) = &op.result {
                    types.insert(result.id, &result.ty);
                }
                if let O::GcSafepoint { stack_map, .. } = &op.kind {
                    let current = stack_map
                        .roots
                        .iter()
                        .filter_map(|r| match r {
                            StackRoot::Value { value, .. }
                                if !dead_values.contains(&(block.id, index, *value)) =>
                            {
                                Some(Slot::Value(*value))
                            }
                            StackRoot::Local { name, .. }
                                if !dead_locals.contains(&(block.id, index, name.clone())) =>
                            {
                                Some(Slot::Local(name.clone()))
                            }
                            _ => None,
                        })
                        .collect();
                    roots.insert((block.id, index), current);
                }
            }
        }
        let mut reachable = forced.clone();
        loop {
            let mut next = reachable.clone();
            for id in &reachable {
                let block = ir.blocks.iter().find(|b| b.id == *id)?;
                next.extend(successors(&block.terminator));
            }
            if next == reachable {
                break;
            }
            reachable = next;
        }
        if !wide && reachable.contains(&ir.failure) {
            return None;
        }
        let mut reachable: Vec<_> = reachable.into_iter().collect();
        reachable.sort_by_key(|b| b.0);
        let predecessors = reachable
            .iter()
            .map(|id| {
                (
                    *id,
                    reachable
                        .iter()
                        .copied()
                        .filter(|p| {
                            successors(&ir.blocks.iter().find(|b| b.id == *p).unwrap().terminator)
                                .contains(id)
                        })
                        .collect(),
                )
            })
            .collect();
        let edge_preserves = ir
            .blocks
            .iter()
            .map(|b| {
                let preserves = match &b.terminator {
                    Terminator::Goto {
                        cleanup_chain,
                        release_scopes,
                        ..
                    } => cleanup_chain.is_empty() && (release_scopes.is_empty() || closed_scopes),
                    _ => true,
                };
                (b.id, preserves)
            })
            .collect();
        Some(Self {
            ir,
            roots,
            types,
            reachable,
            predecessors,
            forced,
            edge_preserves,
            closed_scopes,
        })
    }

    fn transfer(
        &self,
        op: &Operation,
        current: Option<&[Slot]>,
        state: &mut State,
        returns: &HashSet<String>,
    ) -> bool {
        let mut produced = None;
        match &op.kind {
            O::GcSafepoint { .. } => {
                let current = current.expect("each safepoint has its filtered vector");
                let eligible = !current.is_empty() && current.iter().all(|s| state.contains_key(s));
                // Collection can retire every value absent from the new
                // vector. A previous vector is no longer a retention witness.
                *state = current.iter().cloned().map(|s| (s, true)).collect();
                return eligible;
            }
            O::DeclareLocal { name } => {
                state.insert(Slot::Local(name.clone()), false);
            }
            O::StoreLocal { name, value } => {
                let value = state.get(&Slot::Value(*value)).copied();
                let local = Slot::Local(name.clone());
                state.remove(&local);
                if let Some(rooted) = value {
                    state.insert(local, rooted);
                }
            }
            O::LoadLocal { name } => {
                produced = state.get(&Slot::Local(name.clone())).copied();
            }
            O::Constant(
                ConstantValue::Unit
                | ConstantValue::Int(0)
                | ConstantValue::Bool(false)
                | ConstantValue::String(_)
                | ConstantValue::Bytes(_),
            ) => {
                produced = Some(false);
            }
            O::Constant(_) => {}
            // These exact C helpers create or validate same-Data reference
            // results without collecting, retiring old IDs, yielding or
            // reentering. Scalar bits do not acquire a membership fact.
            O::Construct { .. }
            | O::Field { .. }
            | O::EnumPayload { .. }
            | O::DataOperation { .. }
            | O::BytesPad { .. }
            | O::ReadNativeResult { .. } => {
                if op
                    .result
                    .as_ref()
                    .is_some_and(|v| v.ty.is_managed_reference())
                {
                    produced = Some(false);
                }
            }
            O::ManagedCall { target, .. } => {
                // A callee can collect/reenter. Only the unchanged caller
                // frame (or an exact copy of one of its words) keeps a fact.
                state.retain(|_, rooted| *rooted);
                if returns.contains(target) {
                    produced = Some(false);
                }
            }
            O::PrepareNativeCall { .. }
            | O::SourceNativeCall { .. }
            | O::CloseNativeCall { .. }
            | O::SettleNativeAdmissions { .. } => {
                // This also covers nested callbacks. A callback-free local
                // signature is never used as a no-reentry proof.
                state.retain(|_, rooted| *rooted);
            }
            O::Unary { operand, .. } | O::NumericConvert { operand, .. } => {
                if !self
                    .types
                    .get(operand)
                    .is_some_and(|t| t.scalar().is_some())
                {
                    state.retain(|_, rooted| *rooted);
                }
            }
            O::Binary { left, right, .. } => {
                if ![left, right]
                    .iter()
                    .all(|v| self.types.get(v).is_some_and(|t| t.scalar().is_some()))
                {
                    state.retain(|_, rooted| *rooted);
                }
            }
            // Source-native read validated/created and retained the received
            // result. Release only closes callbacks and frees native buffers;
            // lexical storage release does not collect managed identities.
            O::ReleaseNativeCall { .. }
            | O::EnterScope { .. }
            | O::ExitScope { .. }
            | O::Parameter { .. }
            | O::CheckLocal { .. }
            | O::CancellationPoll { .. }
            | O::StoreField { .. }
            | O::CoreLength { .. }
            | O::FunctionReference { .. }
            | O::StaticHandlerReference { .. } => {}
            _ => state.clear(),
        }
        if let Some(result) = &op.result {
            // A loop's re-executed definition must kill its previous word.
            let slot = Slot::Value(result.id);
            state.remove(&slot);
            if let Some(rooted) = produced {
                state.insert(slot, rooted);
            }
        }
        false
    }

    fn analyze(&self, returns: &HashSet<String>) -> Option<(FramePlan, bool)> {
        let mut outgoing = HashMap::<BlockId, State>::new();
        let mut entries = HashMap::<BlockId, State>::new();
        // Entry is unknown even if a backedge targets it. Unvisited loop
        // predecessors are incorporated as soon as they become reachable.
        // The final equations below must include every predecessor.
        let mut converged = false;
        for _ in 0..1000 {
            let mut changed = false;
            for id in &self.reachable {
                let mut states = Vec::new();
                let unknown = State::new();
                if *id == self.ir.entry {
                    states.push(&unknown);
                }
                states.extend(self.predecessors[id].iter().filter_map(|p| outgoing.get(p)));
                let Some(mut state) = meet(&states) else {
                    continue;
                };
                entries.insert(*id, state.clone());
                let block = self.ir.blocks.iter().find(|b| b.id == *id)?;
                for (index, op) in block.operations.iter().enumerate() {
                    self.transfer(
                        op,
                        self.roots.get(&(*id, index)).map(Vec::as_slice),
                        &mut state,
                        returns,
                    );
                }
                if outgoing.get(id) != Some(&state) {
                    outgoing.insert(*id, state);
                    changed = true;
                }
            }
            if !changed {
                converged = true;
                break;
            }
        }
        if !converged || outgoing.len() != self.reachable.len() {
            return None;
        }
        for id in &self.reachable {
            let unknown = State::new();
            let mut states = Vec::new();
            if *id == self.ir.entry {
                states.push(&unknown);
            }
            states.extend(self.predecessors[id].iter().map(|p| &outgoing[p]));
            if entries.get(id) != meet(&states).as_ref() {
                return None;
            }
        }
        let mut plan = FramePlan::new();
        let mut has_return = false;
        let mut valid_returns = true;
        for id in &self.reachable {
            let mut state = entries[id].clone();
            let block = self.ir.blocks.iter().find(|b| b.id == *id)?;
            for (index, op) in block.operations.iter().enumerate() {
                if self.transfer(
                    op,
                    self.roots.get(&(*id, index)).map(Vec::as_slice),
                    &mut state,
                    returns,
                ) {
                    plan.insert((*id, index));
                }
            }
            if let Terminator::Return { value, .. } = &block.terminator {
                has_return = true;
                valid_returns &= value.is_none_or(|v| state.contains_key(&Slot::Value(v)));
            }
        }
        Some((plan, has_return && valid_returns))
    }

    fn wide_block(
        &self,
        id: BlockId,
        mut state: State,
        returns: &HashSet<String>,
    ) -> Option<(State, MembershipMasks)> {
        let block = self.ir.blocks.iter().find(|b| b.id == id)?;
        let mut masks = HashMap::new();
        for (index, op) in block.operations.iter().enumerate() {
            if !self.closed_scopes && matches!(op.kind, O::ExitScope { .. }) {
                state.retain(|_, rooted| *rooted);
            }
            let current = self.roots.get(&(id, index)).map(Vec::as_slice);
            if let Some(current) = current.filter(|roots| roots.len() <= 64) {
                let known = current.iter().enumerate().fold(0_u64, |mask, (i, slot)| {
                    mask | (u64::from(state.contains_key(slot)) << i)
                });
                if known != 0 {
                    masks.insert((id, index), known);
                }
            }
            self.transfer(op, current, &mut state, returns);
        }
        if self.edge_preserves[&id] {
            // Do not import an unpublished producer over any CFG edge.
            state.retain(|_, rooted| *rooted);
        } else {
            state.clear();
        }
        Some((state, masks))
    }

    fn memberships(&self, returns: &HashSet<String>) -> Option<MembershipMasks> {
        let mut outgoing = HashMap::<BlockId, State>::new();
        let mut entries = HashMap::<BlockId, State>::new();
        let mut converged = false;
        for _ in 0..1000 {
            let mut changed = false;
            for id in &self.reachable {
                let unknown = State::new();
                let mut states = Vec::new();
                if self.forced.contains(id) {
                    states.push(&unknown);
                }
                states.extend(self.predecessors[id].iter().filter_map(|p| outgoing.get(p)));
                let Some(state) = meet(&states) else { continue };
                entries.insert(*id, state.clone());
                let (state, _) = self.wide_block(*id, state, returns)?;
                if outgoing.get(id) != Some(&state) {
                    outgoing.insert(*id, state);
                    changed = true;
                }
            }
            if !changed {
                converged = true;
                break;
            }
        }
        if !converged || outgoing.len() != self.reachable.len() {
            return None;
        }
        let mut masks = HashMap::new();
        for id in &self.reachable {
            let unknown = State::new();
            let mut states = Vec::new();
            if self.forced.contains(id) {
                states.push(&unknown);
            }
            states.extend(self.predecessors[id].iter().map(|p| &outgoing[p]));
            if entries.get(id) != meet(&states).as_ref() {
                return None;
            }
            masks.extend(self.wide_block(*id, entries[id].clone(), returns)?.1);
        }
        Some(masks)
    }
}

fn meet(states: &[&State]) -> Option<State> {
    let mut result = (*states.first()?).clone();
    for other in &states[1..] {
        result.retain(|slot, rooted| {
            if let Some(other) = other.get(slot) {
                *rooted &= *other;
                true
            } else {
                false
            }
        });
    }
    Some(result)
}

fn successors(terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Goto { target, .. } => vec![*target],
        Terminator::Branch {
            then_target,
            else_target,
            ..
        } => vec![*then_target, *else_target],
        Terminator::Switch { cases, .. } => cases.iter().map(|(_, b)| *b).collect(),
        _ => Vec::new(),
    }
}

/// Complete nonempty C-runtime vectors with redundant membership validation.
/// This is a private backend effect proof, not a general language optimization.
/// Parameters start unknown; a type alone never proves a root. Every normal
/// CFG predecessor participates, including loops. Closed synchronous return
/// summaries start empty and grow only from independently established facts.
/// Async, catch/finally, indirect and supplant activations keep full checking.
#[must_use]
pub fn proven_frame_roots(module: &IrModule) -> HashMap<String, FramePlan> {
    let functions: Vec<_> = module.functions.iter().filter_map(Function::new).collect();
    let mut returns = HashSet::new();
    for _ in 0..=functions.len() {
        let mut next = returns.clone();
        for f in &functions {
            if f.analyze(&returns)
                .is_some_and(|(_, valid_return)| valid_return)
            {
                next.insert(f.ir.name.clone());
            }
        }
        if next == returns {
            return functions
                .iter()
                .filter_map(|f| {
                    f.analyze(&returns)
                        .map(|(plan, _)| (f.ir.name.clone(), plan))
                })
                .collect();
        }
        returns = next;
    }
    HashMap::new()
}

/// C-only immutable selectors for membership words, including mixed vectors.
/// No proof is stored in a frame/Domain or carried through an unknown entry.
/// All physical publication, collection, Owner/borrow/writing and polls remain.
#[must_use]
pub fn proven_frame_memberships(module: &IrModule) -> HashMap<String, MembershipMasks> {
    let functions: Vec<_> = module.functions.iter().filter_map(Function::new).collect();
    let mut returns = HashSet::new();
    let mut converged = false;
    for _ in 0..=functions.len() {
        let mut next = returns.clone();
        for f in &functions {
            if f.analyze(&returns).is_some_and(|(_, valid)| valid) {
                next.insert(f.ir.name.clone());
            }
        }
        if next == returns {
            converged = true;
            break;
        }
        returns = next;
    }
    if !converged {
        return HashMap::new();
    }
    let closed = super::proven_owner::closed_functions(module);
    module
        .functions
        .iter()
        .filter_map(|ir| {
            Function::wide(ir, closed.contains(&ir.name))?
                .memberships(&returns)
                .map(|plan| (ir.name.clone(), plan))
        })
        .collect()
}
