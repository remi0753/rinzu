//! Normal Error effects, separate from traps, cancellation and runtime faults.
//! No declarations or handwritten throws lists are needed by source programs.
mod flow;
mod language;
mod operations;
mod responsibility;
mod substitution;
mod value;
use crate::ast::*;
use crate::ast_walk::{self, Visitor};
use crate::{CheckedModule, Diagnostic, DiagnosticCode, SourceRange, Type};
use language::{Input, Language, Match, Pattern, Tag};
use std::collections::{BTreeMap, BTreeSet};
use value::{Projection, Value};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Summary {
    pub errors: Language,
    pub success: bool,
    returned: Value,
}

pub(crate) fn analyze(
    checked: &CheckedModule,
    entry: bool,
) -> Result<BTreeMap<String, Summary>, Vec<Diagnostic>> {
    struct Bound(usize);
    impl Visitor for Bound {
        fn enter_statement(&mut self, statement: &Statement) {
            if let StatementKind::TryErrors { handlers, .. } = &statement.kind {
                for handler in handlers {
                    if let CatchPattern::List { elements, .. } = &handler.pattern {
                        self.0 = self.0.max(elements.len());
                    }
                }
            }
        }
    }
    let mut bound = Bound(1);
    ast_walk::module(&mut checked.syntax.clone(), &mut bound);
    let functions: BTreeMap<_, _> = checked
        .functions
        .iter()
        .map(|function| (function.declaration.name.clone(), function))
        .collect();
    let mut summaries = functions
        .keys()
        .map(|name| (name.clone(), Summary::default()))
        .collect::<BTreeMap<_, _>>();
    let order = function_order(&function_graph(checked));
    loop {
        let mut changed = false;
        for name in &order {
            let next = flow::Analyzer::new(checked, &summaries, bound.0, false)
                .function(functions[name])
                .0;
            if summaries[name] != next {
                summaries.insert(name.clone(), next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut diagnostics = Vec::new();
    for function in functions.values() {
        diagnostics.extend(
            flow::Analyzer::new(checked, &summaries, bound.0, true)
                .function(function)
                .1,
        );
    }
    let mut seen = BTreeSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.code.as_str(),
            diagnostic.primary.file_id.0,
            diagnostic.primary.start_byte,
            diagnostic.primary.end_byte,
            diagnostic.message.clone(),
        ))
    });
    if diagnostics.is_empty() {
        diagnostics.extend(responsibility::validate(
            checked, &summaries, bound.0, entry,
        ));
    }
    if diagnostics.is_empty() {
        Ok(summaries)
    } else {
        Err(diagnostics)
    }
}

fn function_graph(checked: &CheckedModule) -> BTreeMap<String, BTreeSet<String>> {
    struct Edges<'a> {
        checked: &'a CheckedModule,
        targets: BTreeSet<String>,
    }
    impl Visitor for Edges<'_> {
        fn expression(&mut self, expression: &mut Expression) {
            let target = match &expression.kind {
                ExpressionKind::Call { target, .. }
                | ExpressionKind::GenericCall { target, .. }
                | ExpressionKind::Spawn { target, .. } => target,
                _ => return,
            };
            self.targets.extend(
                self.checked
                    .indirect_calls
                    .get(&expression.range)
                    .cloned()
                    .unwrap_or_else(|| vec![target.join(".")]),
            );
        }
    }
    checked
        .functions
        .iter()
        .map(|function| {
            let mut edges = Edges {
                checked,
                targets: BTreeSet::new(),
            };
            ast_walk::block(&mut function.declaration.body.clone(), &mut edges);
            (function.declaration.name.clone(), edges.targets)
        })
        .collect()
}

// Visit callees before callers, publishing each new summary immediately. A
// complete unchanged pass still checks the fixed point, including recursion
// and dependencies that are discovered through returned callable values.
fn function_order(graph: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for name in graph.keys() {
        let mut pending = vec![(name.clone(), false)];
        while let Some((name, leaving)) = pending.pop() {
            if leaving {
                order.push(name);
            } else if visited.insert(name.clone()) {
                pending.push((name.clone(), true));
                if let Some(callees) = graph.get(&name) {
                    pending.extend(
                        callees
                            .iter()
                            .rev()
                            .filter(|callee| graph.contains_key(*callee))
                            .map(|callee| (callee.clone(), false)),
                    );
                }
            }
        }
    }
    order
}

fn tag(checked: &CheckedModule, ty: &Type) -> Option<Tag> {
    match ty {
        Type::Error => Some(Tag::Any),
        Type::Named(name)
            if checked
                .source_types
                .get(name)
                .is_some_and(|schema| schema.kind == "error") =>
        {
            Some(Tag::Named(name.clone()))
        }
        _ => None,
    }
}

fn pattern(checked: &CheckedModule, catch: &CatchPattern) -> Pattern {
    match catch {
        CatchPattern::Binding(_) => Pattern::whole(),
        CatchPattern::List { elements, rest, .. } => Pattern {
            prefix: elements
                .iter()
                .map(|element| {
                    tag(checked, &checked.error_patterns[&element.range])
                        .expect("checked Error pattern")
                })
                .collect(),
            rest: rest.is_some(),
        },
    }
}
