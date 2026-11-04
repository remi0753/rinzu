//! Normal Error effects, separate from traps, cancellation and runtime faults.
//! Inference is the default; explicit contracts bound escaping ordinary Errors.
mod contracts;
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

/// Annotation well-formedness does not depend on whether a template is used.
pub(crate) fn validate_annotations(module: &mut Module) -> Result<(), Vec<Diagnostic>> {
    struct Annotations<'a> {
        errors: &'a BTreeMap<String, usize>,
        parameters: BTreeSet<String>,
        diagnostics: Vec<Diagnostic>,
    }
    impl Visitor for Annotations<'_> {
        fn enter_function(&mut self, function: &FunctionDecl) {
            let Some(contract) = &function.error_contract else {
                return;
            };
            let mut seen = BTreeSet::new();
            for ty in &contract.types {
                let (name, count) = match &ty.kind {
                    TypeRefKind::Named(path) => (path.join("."), 0),
                    TypeRefKind::Generic { path, arguments } => (path.join("."), arguments.len()),
                    _ => (String::new(), 0),
                };
                let valid = !self.parameters.contains(&name)
                    && if name == "Error" {
                        count == 0
                    } else {
                        self.errors.get(&name) == Some(&count)
                    };
                if !valid || !seen.insert(crate::interfaces::spelling(ty)) {
                    self.diagnostics.push(Diagnostic::error(DiagnosticCode::InvalidErrorContract,ty.range,"an Error contract requires distinct nominal Error types or Error; a bare type parameter is not an effect variable","use throws E1, E2 with declared Error types, throws Error, or nothrow"));
                }
            }
            if seen.contains("Error") && seen.len() > 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidErrorContract,
                    contract.range,
                    "Error already includes all ordinary Error types",
                    "use throws Error without redundant concrete types",
                ));
            }
        }
    }
    let errors = module
        .declarations
        .iter()
        .filter_map(|d| {
            if let Declaration::Error(e) = d {
                Some((e.name.clone(), e.type_parameters.len()))
            } else {
                None
            }
        })
        .collect();
    let mut visitor = Annotations {
        errors: &errors,
        parameters: BTreeSet::new(),
        diagnostics: Vec::new(),
    };
    for declaration in &mut module.declarations {
        visitor.parameters = match declaration {
            Declaration::Function(f) => f.type_parameters.iter().map(|p| p.name.clone()).collect(),
            Declaration::Interface(i) => i
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .chain(std::iter::once("Self".into()))
                .collect(),
            _ => BTreeSet::new(),
        };
        ast_walk::declaration(declaration, &mut visitor);
    }
    if visitor.diagnostics.is_empty() {
        Ok(())
    } else {
        Err(visitor.diagnostics)
    }
}

pub(crate) fn public_types(summary: &Summary) -> Vec<String> {
    let mut types = BTreeSet::new();
    for tag in summary
        .errors
        .iter()
        .flat_map(|word| word.prefix.iter().chain(&word.tail))
    {
        match tag {
            Tag::Named(name) => {
                types.insert(name.clone());
            }
            Tag::Set(names) => types.extend(names.iter().cloned()),
            Tag::Any | Tag::Parameter(_) | Tag::FutureElement(_, _) => return vec!["Error".into()],
        }
    }
    types.into_iter().collect()
}

pub(crate) fn analyze(
    checked: &CheckedModule,
    entry: bool,
) -> Result<BTreeMap<String, Summary>, Vec<Diagnostic>> {
    let _timing = crate::timing::span("frontend.effects");
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
    let mut timing = crate::timing::span("frontend.effects.summaries");
    let mut evaluated = 0;
    let mut rounds = 0;
    loop {
        rounds += 1;
        let mut changed = false;
        for name in &order {
            evaluated += 1;
            let mut next = flow::Analyzer::new(checked, &summaries, bound.0, false)
                .function(functions[name])
                .0;
            if let Some(errors) =
                contracts::declared(checked, &functions[name].declaration, bound.0)
            {
                next.errors = errors;
            }
            if summaries[name] != next {
                summaries.insert(name.clone(), next);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    timing.set_counter("functions", functions.len() as u64);
    timing.set_counter("evaluated", evaluated);
    timing.set_counter("rounds", rounds);
    drop(timing);
    let mut diagnostics = Vec::new();
    for function in functions.values() {
        let (actual, errors) =
            flow::Analyzer::new(checked, &summaries, bound.0, true).function(function);
        diagnostics.extend(errors);
        diagnostics.extend(contracts::validate(checked, &function.declaration, &actual));
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
