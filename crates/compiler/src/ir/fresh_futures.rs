//! Private C-backend consumption proof. Semantic IR and root vectors stay intact.
use super::{
    BlockId, IrFunction, IrModule, OperationKind as O, Terminator, ValueId,
    operation_cleanup_chain, operation_error_edge, operation_failure_target, operation_inputs,
};
use crate::Type;
use std::collections::{HashMap, HashSet};

fn closed_wrapper(f: &IrFunction) -> bool {
    !f.is_async
        && matches!(f.result_type, Type::Future(_))
        && f.scopes.iter().all(|s| s.deferred_finally.is_empty())
        && f.blocks.iter().all(|b| {
            !matches!(&b.terminator,
                Terminator::Goto { cleanup_chain, .. } | Terminator::Return { cleanup_chain, .. }
                if !cleanup_chain.is_empty())
                && b.operations.iter().all(|op| {
                    op.unwind.as_ref().is_none_or(|u| u.finally.is_empty())
                        && operation_error_edge(&op.kind).is_none_or(|e| e.target == f.failure)
                        && operation_failure_target(&op.kind).is_none_or(|b| b == f.failure)
                        && operation_cleanup_chain(&op.kind).is_none_or(<[BlockId]>::is_empty)
                        && !matches!(
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
                })
        })
}

fn semantic_uses(f: &IrFunction, value: ValueId) -> usize {
    f.blocks
        .iter()
        .flat_map(|b| &b.operations)
        .flat_map(|op| operation_inputs(&op.kind))
        .filter(|v| *v == value)
        .count()
}

fn terminator_uses(f: &IrFunction, value: ValueId) -> usize {
    f.blocks
        .iter()
        .filter(|b| match &b.terminator {
            Terminator::Return { value: v, .. } => *v == Some(value),
            Terminator::Switch { value: v, .. } | Terminator::Branch { condition: v, .. } => {
                *v == value
            }
            _ => false,
        })
        .count()
}

fn fresh(kind: &O, async_functions: &HashSet<&str>, wrappers: &HashSet<&str>) -> bool {
    match kind {
        O::ManagedCall { target, .. } => {
            async_functions.contains(target.as_str()) || wrappers.contains(target.as_str())
        }
        O::SourceNativeCall { signature, .. } => signature.is_async,
        _ => false,
    }
}

fn returns_fresh(
    f: &IrFunction,
    async_functions: &HashSet<&str>,
    wrappers: &HashSet<&str>,
) -> bool {
    let mut reachable = HashSet::from([f.entry]);
    loop {
        let mut next = reachable.clone();
        for b in f.blocks.iter().filter(|b| reachable.contains(&b.id)) {
            match &b.terminator {
                Terminator::Goto { target, .. } => {
                    next.insert(*target);
                }
                Terminator::Branch {
                    then_target,
                    else_target,
                    ..
                } => {
                    next.extend([*then_target, *else_target]);
                }
                Terminator::Switch { cases, .. } => {
                    next.extend(cases.iter().map(|(_, b)| *b));
                }
                _ => {}
            }
        }
        if next == reachable {
            break;
        }
        reachable = next;
    }
    if reachable.contains(&f.failure) {
        return false;
    }
    let mut returned = false;
    for b in f.blocks.iter().filter(|b| reachable.contains(&b.id)) {
        if let Terminator::Return { value, .. } = &b.terminator {
            returned = true;
            let Some(value) = value else {
                return false;
            };
            // Verified IR keeps SSA values block-local. No local/capture,
            // parameter, returned alias or path-dependent identity is trusted.
            let Some(producer) = b
                .operations
                .iter()
                .find(|op| op.result.as_ref().is_some_and(|v| v.id == *value))
            else {
                return false;
            };
            if !fresh(&producer.kind, async_functions, wrappers)
                || semantic_uses(f, *value) != 0
                || terminator_uses(f, *value) != 1
            {
                return false;
            }
        }
    }
    returned
}

/// Additional direct awaits whose synchronous call returns a fresh unaliased
/// Future. Empty initial summaries prevent circular freshness assumptions.
/// All logical admissions, calls, polls and stores remain; the C backend only
/// selects the original consume path after the original await has settled.
#[must_use]
pub fn fresh_return_consuming_awaits(
    module: &IrModule,
) -> HashMap<String, HashSet<(BlockId, usize)>> {
    let async_functions = module
        .functions
        .iter()
        .filter(|f| f.is_async)
        .map(|f| f.name.as_str())
        .collect();
    let candidates: Vec<_> = module
        .functions
        .iter()
        .filter(|f| closed_wrapper(f))
        .collect();
    let mut wrappers = HashSet::new();
    for _ in 0..=candidates.len() {
        let next: HashSet<_> = wrappers
            .iter()
            .copied()
            .chain(
                candidates
                    .iter()
                    .filter(|f| returns_fresh(f, &async_functions, &wrappers))
                    .map(|f| f.name.as_str()),
            )
            .collect();
        if next == wrappers {
            let mut result = HashMap::new();
            for f in &module.functions {
                for b in &f.blocks {
                    for (index, op) in b.operations.iter().enumerate() {
                        let O::Await {
                            future,
                            consume: false,
                            ..
                        } = &op.kind
                        else {
                            continue;
                        };
                        let producer = b
                            .operations
                            .iter()
                            .find(|p| p.result.as_ref().is_some_and(|v| v.id == *future));
                        if producer.is_some_and(|p| matches!(&p.kind, O::ManagedCall {target,..} if wrappers.contains(target.as_str())))
                            && semantic_uses(f, *future) == 1 && terminator_uses(f, *future) == 0 {
                            result.entry(f.name.clone()).or_insert_with(HashSet::new).insert((b.id, index));
                        }
                    }
                }
            }
            return result;
        }
        wrappers = next;
    }
    HashMap::new()
}
