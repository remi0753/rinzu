//! Normal Error effects, separate from traps, cancellation and runtime faults.
//! Inference is the default; explicit contracts bound escaping ordinary Errors.
pub(crate) mod catch_diagnostics;
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
use std::collections::{BTreeMap, BTreeSet, HashSet};
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
    impl Annotations<'_> {
        fn contract(&mut self, contract: &ErrorContract) {
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
                    self.diagnostics.push(Diagnostic::error(DiagnosticCode::InvalidErrorContract,ty.range,"an Error contract requires distinct nominal Error types or Error; a bare type parameter is not an effect variable","use throws(E1, E2) with declared Error types, throws(Error), or nothrow"));
                }
            }
            if seen.contains("Error") && seen.len() > 1 {
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticCode::InvalidErrorContract,
                    contract.range,
                    "Error already includes all ordinary Error types",
                    "use throws(Error) without redundant concrete types",
                ));
            }
        }
    }
    impl Visitor for Annotations<'_> {
        fn enter_function(&mut self, function: &FunctionDecl) {
            if let Some(contract) = &function.error_contract {
                self.contract(contract);
            }
        }

        fn ty(&mut self, ty: &mut TypeRef) {
            if let TypeRefKind::Function {
                error_contract: Some(contract),
                supplant,
                ..
            } = &ty.kind
            {
                if *supplant {
                    self.diagnostics.push(Diagnostic::error(
                        DiagnosticCode::InvalidErrorContract,
                        contract.range,
                        "callback Error contracts apply to managed function types",
                        "use the native supplant signature and observe its Future",
                    ));
                } else {
                    self.contract(contract);
                }
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
            Declaration::TypeAlias(alias) => alias
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            Declaration::Record(record) => record
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            Declaration::Enum(enumeration) => enumeration
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect(),
            Declaration::Error(error) => error
                .type_parameters
                .iter()
                .map(|p| p.name.clone())
                .collect(),
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

pub(crate) fn within_bound(actual: &[String], expected: &[String]) -> bool {
    expected == ["Error"] || actual.iter().all(|name| expected.contains(name))
}

pub(crate) use contracts::display_bound;

#[derive(Clone, Debug)]
pub(crate) struct ErrorConversion {
    pub expected: Vec<String>,
    pub related: Vec<(SourceRange, String)>,
}

pub(crate) fn conversion_error(
    range: SourceRange,
    expected: &[String],
    actual: Option<&[String]>,
    related: Vec<(SourceRange, String)>,
) -> Diagnostic {
    let expected = display_bound(expected);
    let message = actual.map_or_else(
        || format!("cannot establish that the callback's ordinary Errors fit `{expected}`"),
        |actual| {
            format!(
                "callback contract `{}` exceeds expected `{expected}`",
                display_bound(actual),
            )
        },
    );
    let mut diagnostic = Diagnostic::error(
        DiagnosticCode::InvalidErrorContract,
        range,
        message,
        "pass a callback whose ordinary Errors fit the expected bound",
    );
    diagnostic.secondary = related;
    diagnostic
}

pub(crate) fn public_types(summary: &Summary) -> Vec<String> {
    public_error_types(&summary.errors)
}

fn public_error_types(errors: &Language) -> Vec<String> {
    let mut types = BTreeSet::new();
    for tag in errors
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

fn analysis_bound(checked: &CheckedModule) -> usize {
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
    let discarded = checked
        .rethrows
        .values()
        .filter_map(|selection| {
            selection
                .tail
                .map(|start| start as usize - selection.elements.len())
        })
        .max()
        .unwrap_or(0);
    bound.0 + discarded
}

fn infer(
    checked: &CheckedModule,
    functions: &BTreeMap<String, &crate::CheckedFunction>,
    graph: &BTreeMap<String, BTreeSet<String>>,
    bound: usize,
    proof: bool,
) -> BTreeMap<String, Summary> {
    let mut summaries = functions
        .keys()
        .map(|name| (name.clone(), Summary::default()))
        .collect::<BTreeMap<_, _>>();
    let order = function_order(graph);
    let mut timing = crate::timing::span("frontend.effects.summaries");
    let mut evaluated = 0;
    let mut rounds = 0;
    loop {
        rounds += 1;
        let mut changed = false;
        for name in &order {
            evaluated += 1;
            let mut next = flow::Analyzer::new(checked, &summaries, bound, false)
                .proving_conversions(proof)
                .function(functions[name])
                .0;
            if let Some(errors) = contracts::declared(checked, &functions[name].declaration, bound)
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
    summaries
}

fn functions_by_name(checked: &CheckedModule) -> BTreeMap<String, &crate::CheckedFunction> {
    checked
        .functions
        .iter()
        .map(|function| (function.declaration.name.clone(), function))
        .collect()
}

pub(crate) fn verify_conversions(checked: &CheckedModule) -> Result<(), Vec<Diagnostic>> {
    if checked.error_conversions.is_empty() {
        return Ok(());
    }
    let functions = functions_by_name(checked);
    let bound = analysis_bound(checked);
    let summaries = infer(checked, &functions, &function_graph(checked), bound, true);
    let mut seen = HashSet::new();
    let diagnostics: Vec<_> = functions
        .values()
        .flat_map(|function| {
            flow::Analyzer::new(checked, &summaries, bound, true)
                .proving_conversions(true)
                .function(function)
                .1
        })
        .filter(|diagnostic| {
            diagnostic.code == DiagnosticCode::InvalidErrorContract
                && seen.insert(diagnostic.primary)
        })
        .collect();
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EditorEffects {
    pub file: crate::FileId,
    pub awaits: std::collections::HashMap<SourceRange, BTreeSet<String>>,
    pub calls: std::collections::HashMap<SourceRange, BTreeSet<String>>,
}

impl EditorEffects {
    pub fn new(file: crate::FileId) -> Self {
        Self {
            file,
            awaits: Default::default(),
            calls: Default::default(),
        }
    }
}

pub(crate) fn analyze(
    checked: &CheckedModule,
    entry: bool,
    mut editor_effects: Option<&mut EditorEffects>,
    check_responsibility: bool,
) -> Result<BTreeMap<String, Summary>, Vec<Diagnostic>> {
    let _timing = crate::timing::span("frontend.effects");
    let functions = functions_by_name(checked);
    let bound = analysis_bound(checked);
    let graph = function_graph(checked);
    let summaries = infer(checked, &functions, &graph, bound, false);
    let mut failures = Vec::new();
    for function in functions.values() {
        let (actual, errors) = flow::Analyzer::new(checked, &summaries, bound, true)
            .recording(editor_effects.as_deref_mut())
            .function(function);
        let violation = contracts::violation(checked, &function.declaration, &actual);
        if !errors.is_empty() || violation.is_some() {
            failures.push((
                *function,
                errors,
                violation.map(|allowed| (actual, allowed)),
            ));
        }
    }
    if failures.is_empty() {
        // Observation validation consumes these summaries without changing the
        // checked types or Error bounds. Hover can use them before diagnostics finish.
        if check_responsibility {
            let diagnostics = responsibility::validate(checked, &summaries, &graph, bound, entry);
            if !diagnostics.is_empty() {
                return Err(diagnostics);
            }
        }
        return Ok(summaries);
    }

    let mut diagnostics =
        effect_diagnostics(checked, &functions, &graph, &summaries, bound, failures);
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
    Err(diagnostics)
}

type FunctionFailure<'a> = (
    &'a crate::CheckedFunction,
    Vec<Diagnostic>,
    Option<(Summary, BTreeSet<Tag>)>,
);

fn effect_diagnostics(
    checked: &CheckedModule,
    functions: &BTreeMap<String, &crate::CheckedFunction>,
    graph: &BTreeMap<String, BTreeSet<String>>,
    summaries: &BTreeMap<String, Summary>,
    bound: usize,
    failures: Vec<FunctionFailure<'_>>,
) -> Vec<Diagnostic> {
    let mut timing = crate::timing::span("frontend.effects.diagnostics");
    let ordered = failures.iter().any(|(_, diagnostics, _)| {
        diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.code,
                DiagnosticCode::NonExhaustiveCatch | DiagnosticCode::InvalidControlFlow
            )
        })
    });
    let mut traces = BTreeMap::new();
    let mut replays = Vec::new();
    for (function, diagnostics, violation) in &failures {
        let targets: HashSet<_> = diagnostics
            .iter()
            .filter_map(|d| {
                matches!(
                    d.code,
                    DiagnosticCode::NonExhaustiveCatch | DiagnosticCode::InvalidControlFlow
                )
                .then_some(d.primary)
            })
            .collect();
        if targets.is_empty() && violation.is_none() {
            replays.push(None);
            continue;
        }
        let replay = ordered.then(|| catch_diagnostics::Replay::new(targets));
        let allowed = violation
            .as_ref()
            .map_or_else(BTreeSet::new, |(_, allowed)| allowed.clone());
        let (errors, replay) = flow::Analyzer::new(checked, summaries, bound, false)
            .traced_function(function, allowed, replay);
        traces.insert(function.declaration.name.as_str(), errors);
        replays.push(replay);
    }
    timing.set_counter("functions", traces.len() as u64);
    let mut callee_replays = 0;
    let mut expandable = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for ((function, errors, violation), replay) in failures.into_iter().zip(replays) {
        let mut reported = HashSet::new();
        let mut expand = |callee: &str, word: &language::Word| {
            let (name, function) = functions.get_key_value(callee)?;
            if !*expandable.entry(name.as_str()).or_insert_with(|| {
                let source = function.declaration.body.range.file_id;
                source != crate::FileId::MANIFEST
                    && checked.closure_source != Some(source)
                    && source != crate::collections::source_file().id()
                    && !is_recursive(name, graph)
                    && flow::arguments_independent(checked, function)
            }) {
                return None;
            }
            let errors = traces.entry(name.as_str()).or_insert_with(|| {
                callee_replays += 1;
                flow::Analyzer::new(checked, summaries, bound, false)
                    .traced_function(
                        function,
                        BTreeSet::new(),
                        Some(catch_diagnostics::Replay::new(HashSet::new())),
                    )
                    .0
            });
            catch_diagnostics::best_sequence(errors.iter().filter_map(|candidate| {
                let sequence = errors.sequence(candidate)?;
                (sequence.word.guards.is_empty()
                    && sequence
                        .word
                        .prefix
                        .iter()
                        .chain(&sequence.word.tail)
                        .all(|tag| !matches!(tag, Tag::Parameter(_) | Tag::FutureElement(..)))
                    && word.contains(&sequence.word))
                .then_some(sequence)
            }))
            .cloned()
        };
        for mut diagnostic in errors {
            if let Some(replay) = &replay {
                match diagnostic.code {
                    DiagnosticCode::NonExhaustiveCatch => {
                        let at = diagnostic.primary;
                        if !reported.insert(at) {
                            continue;
                        }
                        if let Some(detail) = replay.diagnostic(at, &mut expand) {
                            diagnostic = detail;
                        }
                    }
                    DiagnosticCode::InvalidControlFlow => {
                        replay.explain_rethrow(&mut diagnostic, &mut expand);
                    }
                    _ => {}
                }
            }
            diagnostics.push(diagnostic);
        }
        if let Some((actual, allowed)) = violation {
            let traced = &traces[function.declaration.name.as_str()];
            diagnostics.push(contracts::diagnostic(
                &function.declaration,
                &actual,
                allowed,
                traced,
            ));
        }
    }
    timing.set_counter("callee_replays", callee_replays);
    diagnostics
}

fn is_recursive(name: &str, graph: &BTreeMap<String, BTreeSet<String>>) -> bool {
    let mut visited = BTreeSet::new();
    let mut pending = graph[name].iter().collect::<Vec<_>>();
    while let Some(next) = pending.pop() {
        if next == name {
            return true;
        }
        if visited.insert(next)
            && let Some(edges) = graph.get(next)
        {
            pending.extend(edges);
        }
    }
    false
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
