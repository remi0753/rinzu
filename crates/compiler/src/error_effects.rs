//! Normal Error effects, separate from traps, cancellation and runtime faults.
//! Inference is the default; explicit contracts bound escaping ordinary Errors.
pub(crate) mod catch_diagnostics;
mod contracts;
mod fixed_point;
mod flow;
mod language;
mod operations;
pub(crate) mod responsibility;
mod substitution;
mod value;
use crate::FunctionFrontendCache;
use crate::ast::*;
use crate::ast_walk::{self, Visitor};
use crate::function_cache::{self as memo, Facts, Memo, Memos, Read, SharedSummary};
use crate::{CheckedModule, Diagnostic, DiagnosticCode, SourceRange, Type};
use language::{Input, Language, Match, Pattern, Tag};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use value::{Projection, Value};

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

type EffectSummaries = fixed_point::FixedPoint<Summary>;

fn infer(
    checked: &CheckedModule,
    functions: &BTreeMap<String, &crate::CheckedFunction>,
    graph: &BTreeMap<String, BTreeSet<String>>,
    bound: usize,
    proof: bool,
    mut cache: Option<(&Facts<'_>, &mut Memos<SharedSummary<Summary>>)>,
) -> EffectSummaries {
    let mut summaries = EffectSummaries::new(functions.keys().cloned());
    if cache.is_some() {
        summaries.observe(|name| Read::Error(name.into()));
    }
    let order = function_order(graph);
    let mut timing = crate::timing::span("frontend.effects.summaries");
    let (mut evaluated, mut restored) = (0, 0);
    let mut rounds = 0;
    loop {
        rounds += 1;
        let mut changed = false;
        for name in &order {
            if summaries.reusable(name) {
                continue;
            }
            summaries.begin();
            let compute = || {
                let mut next = flow::Analyzer::new(checked, &summaries, bound, false)
                    .proving_conversions(proof)
                    .function(functions[name])
                    .0;
                if let Some(errors) =
                    contracts::declared(checked, &functions[name].declaration, bound)
                {
                    next.errors = errors;
                }
                next
            };
            if let Some((facts, memos)) = &mut cache {
                let (next, reused) = memo::evaluate(
                    memos,
                    name,
                    || memo::fingerprint((facts.body(name)?, bound)),
                    |read| match read {
                        Read::Error(other) => summaries.fingerprint(other),
                        _ => facts.resolve(read),
                    },
                    None,
                    |read| {
                        if let Read::Error(other) = read {
                            summaries.get(other);
                        }
                    },
                    || SharedSummary::new(compute()),
                );
                evaluated += u64::from(!reused);
                restored += u64::from(reused);
                changed |= summaries.update_shared(name, next);
            } else {
                evaluated += 1;
                changed |= summaries.update(name, compute());
            }
        }
        if !changed {
            break;
        }
    }
    timing.set_counter("functions", functions.len() as u64);
    timing.set_counter("evaluated", evaluated);
    timing.set_counter("restored", restored);
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
    let summaries = infer(
        checked,
        &functions,
        &function_graph(checked),
        bound,
        true,
        None,
    );
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
    editor_effects: Option<&mut EditorEffects>,
    check_responsibility: bool,
    mut cache: Option<&mut FunctionFrontendCache>,
) -> Result<BTreeMap<String, Summary>, Vec<Diagnostic>> {
    let _timing = crate::timing::span("frontend.effects");
    let functions = functions_by_name(checked);
    let bound = analysis_bound(checked);
    let graph = function_graph(checked);
    let facts = cache.is_some().then(|| Facts::new(checked));
    let summaries = infer(
        checked,
        &functions,
        &graph,
        bound,
        false,
        facts
            .as_ref()
            .zip(cache.as_mut().map(|cache| &mut cache.errors)),
    );
    let (mut checks, mut futures) = match cache {
        Some(cache) => (Some(&mut cache.error_checks), Some(&mut cache.futures)),
        None => (None, None),
    };
    let cancelled = AtomicBool::new(false);
    // Both validations read the converged Error summaries. Large modules
    // can do that work concurrently without sharing the memo read trackers.
    let parallel = check_responsibility
        && editor_effects.is_none()
        && functions.len() >= 256
        && std::thread::available_parallelism().is_ok_and(|count| count.get() > 1);
    let concurrent = if parallel {
        let _timing = crate::timing::span("frontend.effects.parallel");
        std::thread::scope(|scope| {
            let graph = &graph;
            let effects = summaries.snapshot();
            let future_cache = futures.as_deref_mut();
            let cancelled = &cancelled;
            let worker = std::thread::Builder::new()
                .name("rinzu-future-check".into())
                .stack_size(8 * 1024 * 1024)
                .spawn_scoped(scope, move || {
                    let facts = future_cache.is_some().then(|| Facts::new(checked));
                    responsibility::validate(
                        checked,
                        &effects,
                        graph,
                        bound,
                        entry,
                        facts.as_ref().zip(future_cache),
                        cancelled,
                    )
                })
                .ok()?;
            let failures = check_error_contracts(
                checked,
                &functions,
                &summaries,
                bound,
                None,
                facts.as_ref().zip(checks.as_deref_mut()),
            );
            if !failures.is_empty() {
                cancelled.store(true, Ordering::Relaxed);
            }
            let diagnostics = worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            Some((failures, diagnostics))
        })
    } else {
        None
    };
    let (failures, responsibility_diagnostics) = concurrent.unwrap_or_else(|| {
        let failures = check_error_contracts(
            checked,
            &functions,
            &summaries,
            bound,
            editor_effects,
            facts.as_ref().zip(checks),
        );
        let diagnostics = if check_responsibility && failures.is_empty() {
            responsibility::validate(
                checked,
                &summaries,
                &graph,
                bound,
                entry,
                facts.as_ref().zip(futures),
                &cancelled,
            )
        } else {
            Vec::new()
        };
        (failures, diagnostics)
    });
    if failures.is_empty() {
        if !responsibility_diagnostics.is_empty() {
            return Err(responsibility_diagnostics);
        }
        return Ok(summaries.into_values());
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

fn check_error_contracts<'a>(
    checked: &'a CheckedModule,
    functions: &BTreeMap<String, &'a crate::CheckedFunction>,
    summaries: &EffectSummaries,
    bound: usize,
    mut editor_effects: Option<&mut EditorEffects>,
    mut cache: Option<(&Facts<'_>, &mut Memos<()>)>,
) -> Vec<FunctionFailure<'a>> {
    let mut checks_timing = crate::timing::span("frontend.effects.checks");
    let (mut evaluated, mut restored) = (0, 0);
    let mut failures = Vec::new();
    let facts = cache.as_ref().map(|(facts, _)| *facts);
    for (name, function) in functions {
        let resolve = |read: &Read| match read {
            Read::Error(other) => summaries.fingerprint(other),
            _ => facts?.resolve(read),
        };
        let mut compute = || {
            let (actual, errors) = flow::Analyzer::new(checked, summaries, bound, true)
                .recording(editor_effects.as_deref_mut())
                .function(function);
            let violation = contracts::violation(checked, &function.declaration, &actual);
            (actual, errors, violation)
        };
        let (actual, errors, violation) = match (
            cache.as_mut(),
            facts.and_then(|facts| memo::fingerprint((facts.body(name)?, bound))),
        ) {
            (Some((_, memos)), Some(input)) => {
                if memos
                    .get(name)
                    .is_some_and(|memo| memo.restore(input, resolve, None).is_some())
                {
                    restored += 1;
                    continue;
                }
                let (result, reads) = memo::recording(compute);
                if result.1.is_empty()
                    && result.2.is_none()
                    && let Some(memo) = Memo::capture(input, reads, (), resolve)
                {
                    memos.insert(name.clone(), memo);
                }
                result
            }
            _ => compute(),
        };
        evaluated += 1;
        if !errors.is_empty() || violation.is_some() {
            failures.push((
                *function,
                errors,
                violation.map(|allowed| (actual, allowed)),
            ));
        }
    }
    checks_timing.set_counter("evaluated", evaluated);
    checks_timing.set_counter("restored", restored);
    drop(checks_timing);
    failures
}

fn effect_diagnostics(
    checked: &CheckedModule,
    functions: &BTreeMap<String, &crate::CheckedFunction>,
    graph: &BTreeMap<String, BTreeSet<String>>,
    summaries: &EffectSummaries,
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
                    .fact_indirect_calls(&expression.range)
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
                .memo_schema(name)
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
                    tag(
                        checked,
                        checked
                            .fact_error_patterns(&element.range)
                            .expect("checked error patterns"),
                    )
                    .expect("checked Error pattern")
                })
                .collect(),
            rest: rest.is_some(),
        },
    }
}
