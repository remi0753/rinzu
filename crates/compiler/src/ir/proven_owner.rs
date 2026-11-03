//! Successful same-Data C-runtime Owner checks, never cached thread identity.
use super::{
    BlockId, ConstantValue, IrFunction, IrModule, Operation, OperationKind as O, Terminator,
    ValueId, operation_cleanup_chain, operation_error_edge, operation_failure_target,
    virtual_bytes_options,
};
use std::collections::{HashMap, HashSet};

type Plan = HashSet<(BlockId, usize)>;

fn runtime_only(kind: &O) -> bool {
    matches!(
        kind,
        O::Parameter { .. }
            | O::CheckLocal { .. }
            | O::CancellationPoll { .. }
            | O::DeclareLocal { .. }
            | O::LoadLocal { .. }
            | O::StoreLocal { .. }
            | O::Constant(_)
            | O::GcSafepoint { .. }
            | O::Construct { .. }
            | O::DataOperation { .. }
            | O::Field { .. }
            | O::EnumPayload { .. }
            | O::BytesPad { .. }
            | O::StoreField { .. }
            | O::EnterScope { .. }
            | O::ExitScope { .. }
            | O::FunctionReference { .. }
            | O::StaticHandlerReference { .. }
            | O::Binary { .. }
            | O::Unary { .. }
            | O::NumericConvert { .. }
            | O::ThrowError { .. }
            | O::ErrorCast { .. }
            | O::EnterTry { .. }
            | O::LeaveTry { .. }
            | O::CatchErrors { .. }
            | O::ErrorMatch { .. }
            | O::ErrorIndex { .. }
            | O::ErrorRest { .. }
    )
}

// These closed synchronous functions retain no C ownership across operations.
// Recursive activations have a different roots depth.
pub(super) fn closed_functions(module: &IrModule) -> HashSet<String> {
    let mut closed: HashSet<_> = module
        .functions
        .iter()
        .filter(|f| {
            !f.is_async
                && f.blocks
                    .iter()
                    .flat_map(|b| &b.operations)
                    .all(|op| runtime_only(&op.kind) || matches!(op.kind, O::ManagedCall { .. }))
        })
        .map(|f| f.name.clone())
        .collect();
    loop {
        let next: HashSet<_> = module
            .functions
            .iter()
            .filter(|f| closed.contains(&f.name))
            .filter(|f| {
                f.blocks.iter().flat_map(|b| &b.operations).all(|op| {
                !matches!(&op.kind, O::ManagedCall { target, .. } if !closed.contains(target))
            })
            })
            .map(|f| f.name.clone())
            .collect();
        if next == closed {
            return closed;
        }
        closed = next;
    }
}

fn checked(kind: &O, virtuals: &HashSet<ValueId>) -> bool {
    // A virtual projection emits no check. It must not invent authority.
    if matches!(kind, O::EnumPayload { operand, .. } if virtuals.contains(operand)) {
        return false;
    }
    matches!(
        kind,
        O::GcSafepoint { .. }
            | O::Construct { .. }
            | O::DataOperation { .. }
            | O::Field { .. }
            | O::EnumPayload { .. }
    )
}

fn transfer(
    kind: &O,
    known: bool,
    name: &str,
    closed: &HashSet<String>,
    virtuals: &HashSet<ValueId>,
) -> bool {
    if checked(kind, virtuals) {
        return true;
    }
    match kind {
        O::Constant(ConstantValue::String(_) | ConstantValue::Bytes(_))
        | O::BytesPad { .. }
        | O::StoreField { .. } => true,
        O::ManagedCall { target, .. } if closed.contains(target) => known,
        O::ExitScope { .. } if closed.contains(name) => known,
        O::Parameter { .. }
        | O::CheckLocal { .. }
        | O::CancellationPoll { .. }
        | O::DeclareLocal { .. }
        | O::LoadLocal { .. }
        | O::StoreLocal { .. }
        | O::Constant(_)
        | O::EnterScope { .. }
        | O::FunctionReference { .. }
        | O::StaticHandlerReference { .. }
        | O::Binary { .. }
        | O::Unary { .. }
        | O::NumericConvert { .. }
        | O::ErrorCast { .. }
        | O::ErrorMatch { .. }
        | O::ErrorIndex { .. }
        | O::ErrorRest { .. } => known,
        O::EnumPayload { operand, .. } if virtuals.contains(operand) => known,
        // All native/task/await/cleanup and unknown operations discard facts.
        _ => false,
    }
}

pub(super) fn exceptional(op: &Operation, forced: &mut HashSet<BlockId>) {
    if let Some(target) = operation_failure_target(&op.kind) {
        forced.insert(target);
    }
    if let Some(edge) = operation_error_edge(&op.kind) {
        forced.insert(edge.target);
        forced.extend(&edge.cleanup_chain);
    }
    if let Some(chain) = operation_cleanup_chain(&op.kind) {
        forced.extend(chain);
    }
    if let Some(unwind) = &op.unwind {
        forced.extend(&unwind.finally);
    }
    match &op.kind {
        O::EnterTry { cleanup }
        | O::LeaveTry { cleanup }
        | O::TaskScopeOutcome { cleanup, .. }
        | O::ErrorGroupOutcome { cleanup, .. } => {
            forced.insert(*cleanup);
        }
        O::ExitScope {
            deferred_finally, ..
        } => forced.extend(deferred_finally),
        _ => {}
    }
}

fn analyze(function: &IrFunction, closed: &HashSet<String>) -> Option<Plan> {
    let blocks: HashMap<_, _> = function.blocks.iter().map(|b| (b.id, b)).collect();
    let mut successors = HashMap::new();
    let mut stable = HashMap::new();
    let mut forced = HashSet::from([function.entry, function.failure]);
    for block in &function.blocks {
        let mut preserves = true;
        let next = match &block.terminator {
            Terminator::Goto {
                target,
                cleanup_chain,
                release_scopes,
                ..
            } => {
                forced.extend(cleanup_chain);
                preserves = cleanup_chain.is_empty()
                    && (release_scopes.is_empty() || closed.contains(&function.name));
                vec![*target]
            }
            Terminator::Branch {
                then_target,
                else_target,
                ..
            } => vec![*then_target, *else_target],
            Terminator::Switch { cases, .. } => cases.iter().map(|(_, target)| *target).collect(),
            Terminator::Return { cleanup_chain, .. } => {
                forced.extend(cleanup_chain);
                Vec::new()
            }
            _ => Vec::new(),
        };
        if next.iter().any(|id| !blocks.contains_key(id)) {
            return None;
        }
        successors.insert(block.id, next);
        stable.insert(block.id, preserves);
        for op in &block.operations {
            exceptional(op, &mut forced);
        }
    }
    if forced.iter().any(|id| !blocks.contains_key(id)) {
        return None;
    }
    let mut reachable = forced.clone();
    loop {
        let mut next = reachable.clone();
        for id in &reachable {
            next.extend(&successors[id]);
        }
        if next == reachable {
            break;
        }
        reachable = next;
    }
    let predecessors: HashMap<_, Vec<_>> = blocks
        .keys()
        .map(|id| {
            (
                *id,
                successors
                    .iter()
                    .filter(|(p, next)| reachable.contains(*p) && next.contains(id))
                    .map(|(p, _)| *p)
                    .collect(),
            )
        })
        .collect();
    let virtuals = virtual_bytes_options(function);
    let mut incoming: HashMap<_, _> = blocks
        .keys()
        .map(|id| {
            (
                *id,
                reachable.contains(id) && !forced.contains(id) && !predecessors[id].is_empty(),
            )
        })
        .collect();
    let mut outgoing: HashMap<_, _> = blocks
        .iter()
        .map(|(id, block)| {
            (
                *id,
                block.operations.iter().fold(incoming[id], |known, op| {
                    transfer(&op.kind, known, &function.name, closed, &virtuals)
                }),
            )
        })
        .collect();
    // Greatest fixed point of a must analysis: real entry/exceptional facts
    // start false; top on other reachable nodes retains facts through loops.
    for _ in 0..=blocks.len() * 2 + 4 {
        let next_in: HashMap<_, _> = blocks
            .keys()
            .map(|id| {
                (
                    *id,
                    reachable.contains(id)
                        && !forced.contains(id)
                        && !predecessors[id].is_empty()
                        && predecessors[id].iter().all(|p| outgoing[p] && stable[p]),
                )
            })
            .collect();
        let next_out: HashMap<_, _> = blocks
            .iter()
            .map(|(id, block)| {
                (
                    *id,
                    block.operations.iter().fold(next_in[id], |known, op| {
                        transfer(&op.kind, known, &function.name, closed, &virtuals)
                    }),
                )
            })
            .collect();
        if next_in == incoming && next_out == outgoing {
            let mut plan = HashSet::new();
            for block in &function.blocks {
                let mut known = incoming[&block.id];
                for (index, op) in block.operations.iter().enumerate() {
                    if known && reachable.contains(&block.id) && checked(&op.kind, &virtuals) {
                        plan.insert((block.id, index));
                    }
                    known = transfer(&op.kind, known, &function.name, closed, &virtuals);
                }
            }
            return Some(plan);
        }
        incoming = next_in;
        outgoing = next_out;
    }
    None
}

/// Private C entries dominated by a successful same-Data Owner check on every
/// normal path. Public generic helpers and all non-Owner checks are unchanged.
#[must_use]
pub fn proven_data_owner_operations(module: &IrModule) -> HashMap<String, Plan> {
    let closed = closed_functions(module);
    module
        .functions
        .iter()
        .filter_map(|f| analyze(f, &closed).map(|plan| (f.name.clone(), plan)))
        .collect()
}
