//! Observation obligations are task identities, separate from Error effects.
//! Path states retain obligations even after an alias or aggregate is widened.
mod boundaries;
mod calls;
mod contract_cache;
mod data_contracts;
mod expressions;
mod families;
mod flow;
mod iteration;
mod mutations;
mod objects;
mod receivers;
mod retention;
mod shared_map;
mod synchronous;
mod value;
mod watches;
use super::*;
use families::FutureMembers;
use value::{
    Access, FutureCase, FutureResult, Key, Parameter, Site, Token, Tracked, choice, range, site,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum RequirementKind {
    Observation,
    Worker,
    Native,
    NativeInput,
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Requirement {
    kind: RequirementKind,
    origin: Site,
    exit: Site,
    errors: Language,
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Obligation {
    origin: Site,
    errors: Language,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct State {
    env: shared_map::SharedMap<String, Tracked>,
    pending: BTreeMap<Token, Obligation>,
    observed: BTreeSet<Parameter>,
    requirements: Vec<Requirement>,
    restrictions: BTreeSet<retention::Restriction>,
    guards: language::Guards,
    groups: Vec<Group>,
    resolved: BTreeMap<Token, BTreeSet<usize>>,
    iterated: BTreeSet<(Site, Vec<Access>)>,
    /// Variant facts for one representative element, never a whole population.
    iteration_variants: BTreeMap<(Site, Vec<Access>), BTreeSet<String>>,
    map_appends: BTreeSet<Site>,
    objects: shared_map::SharedMap<Token, objects::Object>,
    async_mutable: BTreeSet<Token>,
    sticky_mutable: BTreeSet<Token>,
    unknown_async_mutation: bool,
    mutations: BTreeMap<Token, mutations::Mutation>,
    mutable_inputs: bool,
    stores: BTreeMap<Parameter, Tracked>,
    async_stores: BTreeMap<Parameter, BTreeSet<Token>>,
    watches: BTreeMap<Token, watches::Watch>,
    receivers: BTreeMap<Token, receivers::Receiver>,
    receiver_controls: BTreeSet<Token>,
    receiver_scope_control: bool,
    field_watch_blocked: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Group {
    errors: Language,
    observed: BTreeSet<Token>,
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum Control {
    Normal,
    Diverges,
    Panic,
    Recurse(BTreeSet<Token>),
    Return(Tracked),
    Break,
    Continue,
    Error(language::Word, Option<Token>),
}
#[derive(Clone, Debug, Eq, PartialEq)]
enum Stop {
    Error(Box<language::Word>, Option<Token>),
    Diverges,
    Panic,
    Recurse(BTreeSet<Token>),
}
impl Stop {
    fn control(self) -> Control {
        match self {
            Self::Error(word, token) => Control::Error(*word, token),
            Self::Diverges => Control::Diverges,
            Self::Panic => Control::Panic,
            Self::Recurse(held) => Control::Recurse(held),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Path {
    state: State,
    control: Control,
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Exit {
    guards: language::Guards,
    result: Result<Tracked, Stop>,
    observed: BTreeSet<Parameter>,
    direct_observed: BTreeSet<Parameter>,
    receivers: BTreeMap<Token, receivers::Receiver>,
    receiver_controls: BTreeSet<Token>,
    receiver_scope_control: bool,
    requirements: Vec<Requirement>,
    restrictions: BTreeSet<retention::Restriction>,
    stores: BTreeMap<Parameter, Tracked>,
    saved: BTreeMap<Token, Obligation>,
    async_stores: BTreeMap<Parameter, BTreeSet<Token>>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Contract {
    exits: Vec<Exit>,
    mutates: bool,
    mutates_async: bool,
    exports_residual_mutations: bool,
    disrupts_observers: bool,
}

// Error contracts are checked by the type-effect analysis. Responsibility
// observes bounded list shapes, not suffix types. Erase that extra metadata
// here so independent optional failures retain the existing finite state space.
fn concatenate(left: &Language, right: &Language, bound: usize) -> Language {
    language::concatenate(left, right, bound)
        .into_iter()
        .map(|mut word| {
            word.tail.clear();
            word
        })
        .collect()
}

pub(super) fn validate(
    checked: &CheckedModule,
    effects: &BTreeMap<String, Summary>,
    bound: usize,
    entry: bool,
) -> Vec<Diagnostic> {
    let _timing = crate::timing::span("frontend.responsibility");
    let graph = super::function_graph(checked);
    let recursive = recursive_functions(&graph);
    let order = super::function_order(&graph);
    let functions: BTreeMap<_, _> = checked
        .functions
        .iter()
        .map(|function| (function.declaration.name.clone(), function))
        .collect();
    let contexts = std::cell::RefCell::new(BTreeMap::new());
    let data_functions = data_contracts::functions(checked);
    let mut contracts = contract_cache::Contracts::new(
        checked
            .functions
            .iter()
            .map(|function| function.declaration.name.clone()),
    );
    loop {
        let mut changed = false;
        for name in &order {
            // Track actual reads, including returned callable values and
            // contextual synchronous calls, rather than trusting graph edges.
            if contracts.reusable(name) {
                continue;
            }
            contracts.begin();
            let function = functions[name];
            let mut engine = Engine {
                checked,
                effects,
                contracts: &contracts,
                bound,
                current: &function.declaration.name,
                recursive_calls: std::cell::Cell::new(false),
                non_decreasing: std::cell::Cell::new(false),
                cyclic: recursive.contains(&function.declaration.name),
                mutates: std::cell::Cell::new(false),
                mutates_async: std::cell::Cell::new(false),
                disrupts_observers: std::cell::Cell::new(false),
                contexts: &contexts,
                data_functions: &data_functions,
                context: 0,
                stack: vec![function.declaration.name.clone()],
                allow_field_watch: false,
            };
            // A summary cannot drop a field lease when its class may escape.
            // Contextual synchronous calls keep the actual heap and leases.
            engine.allow_field_watch = !function
                .parameter_types
                .iter()
                .chain(std::iter::once(&function.result_type))
                .any(|ty| engine.contains_mutable_class(ty, &mut BTreeSet::new()));
            let mut next = engine.function(function);
            if let Some(errors) = super::contracts::declared(checked, &function.declaration, bound)
            {
                let origins = next.exits.clone();
                for exit in origins {
                    for word in &errors {
                        let mut exit = exit.clone();
                        exit.result = Err(Stop::Error(Box::new(word.clone()), None));
                        if !next.exits.contains(&exit) {
                            next.exits.push(exit);
                        }
                    }
                }
            }
            if contracts.update(name, next) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut diagnostics = Vec::new();
    if entry
        && let Some(contract) = contracts.get("main")
        && contract
            .exits
            .iter()
            .any(|exit| matches!(exit.result, Err(Stop::Error(..))))
    {
        let main = checked
            .functions
            .iter()
            .find(|function| function.declaration.name == "main")
            .expect("checked Main entry");
        diagnostics.push(Diagnostic::error(
            DiagnosticCode::ErrorEscapesBoundary, main.declaration.body.range,
            "a normal Error can escape the Main entry",
            "catch every normal Error before Main returns, including callees, awaited failures, catch handlers and finally",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut seen_requirements = BTreeSet::new();
    for (name, contract) in contracts.iter() {
        for exit in &contract.exits {
            for restriction in &exit.restrictions {
                if restriction.forbidden && seen.insert((restriction.origin, restriction.origin)) {
                    let message = match restriction.boundary {
                        retention::Boundary::ErrorPayload => {
                            "Error payload cannot retain Future, including through function captures"
                        }
                        retention::Boundary::Native => {
                            "Future or Error cannot cross a native boundary through retained values"
                        }
                        retention::Boundary::NativeCapture => {
                            "native callback capture cannot retain Future"
                        }
                    };
                    diagnostics.push(Diagnostic::error(
                        DiagnosticCode::TypeMismatch,
                        range(restriction.origin),
                        message,
                        "pass an owned value whose reachable retained data satisfies this boundary",
                    ));
                }
            }
            for requirement in &exit.requirements {
                let unconditional: Language = requirement
                    .errors
                    .iter()
                    .filter(|word| !symbolic(word, name))
                    .cloned()
                    .collect();
                if unconditional.is_empty()
                    || !seen_requirements.insert((
                        requirement.kind,
                        requirement.origin,
                        requirement.exit,
                    ))
                {
                    continue;
                }
                let (code, message, help, secondary) = match requirement.kind {
                    RequirementKind::Observation => (
                        DiagnosticCode::FutureDiscarded,
                        "this task can fail after its observation path is lost",
                        "await an alias on every managed exit, return the Future, or pass it to a proven observer",
                        "this exit does not preserve an observation path",
                    ),
                    RequirementKind::Worker => (
                        DiagnosticCode::ErrorEscapesBoundary,
                        "a normal Error can escape this Worker root",
                        "catch every normal Error inside the spawned function, including callees and finally",
                        "the Worker root must complete without an uncaught normal Error",
                    ),
                    RequirementKind::Native => (
                        DiagnosticCode::ErrorEscapesBoundary,
                        "a normal Error can escape this native callback entry",
                        "catch every normal Error inside the callback or provide its explicit ABI-compatible fallback",
                        "the native entry includes its callees, catch handlers and finally",
                    ),
                    RequirementKind::NativeInput => (
                        DiagnosticCode::ErrorEscapesBoundary,
                        "native callback input conversion can fail before the handler runs",
                        "provide an explicit ABI-compatible callback fallback to handle invalid native input",
                        "a catch inside the callback cannot handle its adapter's input conversion",
                    ),
                };
                diagnostics.push(
                    Diagnostic::error(code, range(requirement.origin), message, help)
                        .with_secondary(range(requirement.exit), secondary),
                );
            }
        }
    }
    diagnostics
}
fn symbolic(word: &language::Word, owner: &str) -> bool {
    word.guards.keys().any(|input|input.owner==owner) || word.prefix.iter().any(|tag|matches!(tag,Tag::Parameter(input)|Tag::FutureElement(input,_) if input.owner==owner))
}
fn recursive_functions(graph: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    graph
        .keys()
        .filter(|name| {
            let mut visited = BTreeSet::new();
            let mut pending = graph[*name].iter().cloned().collect::<Vec<_>>();
            while let Some(next) = pending.pop() {
                if next == **name {
                    return true;
                }
                if visited.insert(next.clone())
                    && let Some(edges) = graph.get(&next)
                {
                    pending.extend(edges.iter().cloned());
                }
            }
            false
        })
        .cloned()
        .collect()
}
struct Engine<'a> {
    checked: &'a CheckedModule,
    effects: &'a BTreeMap<String, Summary>,
    contracts: &'a contract_cache::Contracts,
    bound: usize,
    current: &'a str,
    recursive_calls: std::cell::Cell<bool>,
    non_decreasing: std::cell::Cell<bool>,
    cyclic: bool,
    mutates: std::cell::Cell<bool>,
    mutates_async: std::cell::Cell<bool>,
    disrupts_observers: std::cell::Cell<bool>,
    contexts: &'a std::cell::RefCell<BTreeMap<(u32, Site), u32>>,
    data_functions: &'a BTreeSet<String>,
    context: u32,
    stack: Vec<String>,
    allow_field_watch: bool,
}
impl Engine<'_> {
    fn at(&self, range: SourceRange) -> Site {
        let mut at = site(range);
        at.4 = self.context;
        at
    }
    fn private_environment(&self, ty: &str) -> bool {
        let Some(file) = self.checked.closure_source else {
            return false;
        };
        self.checked.syntax.declarations.iter().any(|declaration| {
            let Declaration::Record(record) = declaration else {
                return false;
            };
            record.name == ty
                && record.range.file_id == file
                && record.fields.len() == 1
                && record.fields[0].name == "environment"
        })
    }
    fn immutable_type(&self, ty: &str) -> bool {
        self.checked.source_types.get(ty).is_some_and(|schema| {
            schema.kind == "record"
                || (schema.kind == "state"
                    && schema.const_fields.len() == schema.fields.len()
                    && !self.contains_mutable_class(&Type::Named(ty.into()), &mut BTreeSet::new()))
        }) || self.private_environment(ty)
    }
    fn typed(&self, ty: &Type) -> Value {
        if let Some(tag) = tag(self.checked, ty) {
            return Value::Error(BTreeSet::from([tag]));
        }
        match ty {
            Type::Future(inner) => Value::Future {
                errors: language::unknown(self.bound),
                success: true,
                value: Box::new(self.typed(inner)),
            },
            Type::Bool => Value::Bool(None),
            Type::Never => Value::Bottom,
            Type::Named(name) => Value::Opaque(name.clone()),
            Type::List(inner) | Type::Map(_, inner) => {
                Value::Collection(Box::new(self.typed(inner)))
            }
            _ => Value::Plain,
        }
    }
    fn type_at(&self, range: SourceRange) -> Type {
        self.checked
            .error_upcasts
            .get(&range)
            .or_else(|| self.checked.expression_type(range))
            .cloned()
            .unwrap_or(Type::Never)
    }
    fn pure(&self, range: SourceRange) -> Tracked {
        Tracked::data(self.typed(&self.type_at(range)))
    }
    fn function(&self, function: &crate::CheckedFunction) -> Contract {
        let name = &function.declaration.name;
        let mut state = State::default();
        for (index, (parameter, ty)) in function
            .declaration
            .parameters
            .iter()
            .zip(&function.parameter_types)
            .enumerate()
        {
            state.env.insert(
                parameter.name.clone(),
                Tracked::Input {
                    parameter: Parameter {
                        owner: name.clone(),
                        index,
                        path: Vec::new(),
                    },
                    effect: Value::Parameter {
                        owner: name.clone(),
                        index,
                        path: Vec::new(),
                        fallback: Box::new(self.typed(ty)),
                    },
                    future: matches!(ty, Type::Future(_)),
                },
            );
        }
        let mut exits = Vec::new();
        let mut residual_mutation = false;
        let mut exports_residual_mutations = true;
        for path in self.block(
            &function.declaration.body,
            vec![Path {
                state,
                control: Control::Normal,
            }],
            false,
        ) {
            let protected = if let Control::Recurse(held) = &path.control {
                held.clone()
            } else {
                BTreeSet::new()
            };
            let (mut returned, error) = match path.control {
                Control::Normal => (Some(Tracked::data(Value::Plain)), None),
                Control::Return(value) => (Some(value), None),
                Control::Error(word, token) => (None, Some(Stop::Error(Box::new(word), token))),
                Control::Diverges => (None, Some(Stop::Diverges)),
                Control::Panic => (None, Some(Stop::Panic)),
                Control::Recurse(held) => (None, Some(Stop::Recurse(held))),
                Control::Break | Control::Continue => continue,
            };
            let mut state = path.state;
            // Panic unwinds the whole managed process, cancelling and joining
            // its tasks. It cannot abandon a failure on a continuing path.
            // Keep retention and native-boundary checks from finally blocks.
            let panicking = matches!(error, Some(Stop::Panic));
            if panicking {
                state
                    .requirements
                    .retain(|requirement| requirement.kind != RequirementKind::Observation);
            }
            residual_mutation |= state.unknown_async_mutation
                || !state.mutations.is_empty()
                || !state.sticky_mutable.is_empty();
            let mut stores: BTreeMap<_, _> = state
                .stores
                .iter()
                .map(|(field, value)| {
                    let mut value = state.freeze(value);
                    value.prepare_return(&state.resolved, &state.pending, &state.task_population());
                    value = value.finite();
                    if self.cyclic {
                        value.widen_recursive_return();
                    }
                    (field.clone(), value)
                })
                .collect();
            if let Some(mut value) = returned.take() {
                value = state.freeze(&value);
                state.prepare_mutation_return(&mut value);
                state.sanitize_watched_observations(&mut value);
                value.prepare_return(&state.resolved, &state.pending, &state.task_population());
                returned = Some(value.finite());
            }
            if self.cyclic
                && let Some(value) = &mut returned
            {
                value.widen_recursive_return();
            }
            exports_residual_mutations &= state.exports_mutations(returned.as_ref());
            let mut escaped = returned
                .as_ref()
                .map(Tracked::guaranteed)
                .unwrap_or_default();
            if let Some(value) = &returned {
                escaped.extend(state.deferred_guarantees(value));
            }
            // Potential external storage is a synchronous calling obligation,
            // not a proof of observation. Actual aliases are checked at calls.
            let mut potential: BTreeSet<_> = stores.values().flat_map(Tracked::tokens).collect();
            potential.extend(state.async_stores.values().flatten().cloned());
            let saved: BTreeMap<_, _> = state
                .pending
                .iter()
                .filter(|(token, _)| potential.contains(*token))
                .map(|(token, obligation)| (token.clone(), obligation.clone()))
                .collect();
            escaped.extend(potential);
            for (token, obligation) in &state.pending {
                if !panicking && !escaped.contains(token) && !protected.contains(token) {
                    let requirement = Requirement {
                        kind: RequirementKind::Observation,
                        origin: obligation.origin,
                        exit: self.at(function.declaration.body.range),
                        errors: language::restricted(&obligation.errors, &state.guards),
                    };
                    if !state.requirements.contains(&requirement) {
                        state.requirements.push(requirement);
                    }
                }
            }
            let public = returned
                .iter()
                .chain(stores.values())
                .flat_map(mutations::public_futures)
                .collect();
            let (receiver_summary, receiver_controls) =
                state.fold_receiver_summary(&public, self.cyclic);
            let mut slots = BTreeMap::new();
            let mut objects = BTreeMap::new();
            for value in returned.iter_mut().chain(stores.values_mut()) {
                // An opaque shape cannot transfer owned obligations; those
                // have already become requirements above. Retaining fresh
                // identities inside it would grow recursive summaries without
                // supplying an observation path to the caller.
                value.erase_opaque_local_tokens();
                value.visit_tokens(&mut |token, pending| {
                    *pending = state.pending.contains_key(token);
                    *token = receivers::normalize(token, &mut slots);
                });
                value.visit_objects(&mut |identity| {
                    if matches!(identity, Token::Local(..)) {
                        let next = objects.len() as u32;
                        let family = !identity.observable();
                        let slot = *objects.entry(identity.clone()).or_insert(next);
                        *identity = if family {
                            Token::OutputFamily(slot)
                        } else {
                            Token::Output(slot)
                        };
                    }
                });
            }
            let saved = saved
                .into_iter()
                .map(|(token, obligation)| {
                    let token = if matches!(token, Token::Local(..) | Token::Family(_)) {
                        let next = slots.len() as u32;
                        let family = matches!(token, Token::Local(_, _, true) | Token::Family(_));
                        let slot = *slots.entry(token).or_insert(next);
                        if family {
                            Token::OutputFamily(slot)
                        } else {
                            Token::Output(slot)
                        }
                    } else {
                        token
                    };
                    (token, obligation)
                })
                .collect();
            let async_stores = state
                .async_stores
                .iter()
                .map(|(field, tokens)| {
                    let tokens = tokens
                        .iter()
                        .map(|token| {
                            if matches!(token, Token::Local(..) | Token::Family(_)) {
                                let next = slots.len() as u32;
                                let slot = *slots.entry(token.clone()).or_insert(next);
                                if token.observable() {
                                    Token::Output(slot)
                                } else {
                                    Token::OutputFamily(slot)
                                }
                            } else {
                                token.clone()
                            }
                        })
                        .collect();
                    (field.clone(), tokens)
                })
                .collect();
            let direct_observed = state.direct_observed();
            let receivers = receiver_summary
                .iter()
                .map(|(owner, receiver)| {
                    let owner = receivers::normalize(owner, &mut slots);
                    let mut receiver = receiver.clone();
                    receiver.map_tokens(&mut |token| receivers::normalize(token, &mut slots));
                    (owner, receiver)
                })
                .collect();
            let receiver_controls = receiver_controls
                .iter()
                .map(|token| receivers::normalize(token, &mut slots))
                .collect();
            let exit = Exit {
                guards: state.guards,
                result: returned.map_or_else(|| Err(error.unwrap()), Ok),
                observed: state.observed,
                direct_observed,
                receivers,
                receiver_controls,
                receiver_scope_control: state.receiver_scope_control,
                requirements: state.requirements,
                restrictions: state.restrictions,
                stores,
                saved,
                async_stores,
            };
            if !exits.contains(&exit) {
                exits.push(exit);
            }
        }
        if self.recursive_calls.get() && !self.non_decreasing.get() {
            // A recursive call through a strict immutable subobject cannot
            // traverse a cycle constructed by source programs. Explicit loops
            // and unrelated suspension remain separate divergent outcomes.
            exits.retain(|exit| !matches!(exit.result, Err(Stop::Recurse(_))));
        }
        if data_contracts::immutable_result(self.checked, &function.result_type) {
            let mut joined: Vec<(Exit, Exit)> = Vec::new();
            for exit in exits {
                let mut key = exit.clone();
                if key.result.is_ok() {
                    key.result = Ok(Tracked::data(Value::Plain));
                }
                if let Some((_, previous)) = joined.iter_mut().find(|(saved, _)| *saved == key) {
                    if let (Ok(left), Ok(right)) = (&mut previous.result, exit.result) {
                        left.join(right);
                    }
                } else {
                    joined.push((key, exit));
                }
            }
            exits = joined.into_iter().map(|(_, exit)| exit).collect();
        }
        Contract {
            exits,
            mutates: self.mutates.get(),
            mutates_async: self.mutates_async.get() && residual_mutation,
            exports_residual_mutations,
            disrupts_observers: self.disrupts_observers.get(),
        }
    }
}
impl State {
    fn age(&mut self, stable: &BTreeSet<Token>) {
        let mut tokens: BTreeSet<_> = self.pending.keys().cloned().collect();
        tokens.extend(self.object_tokens());
        tokens.extend(self.watch_tokens());
        tokens.extend(self.receiver_tokens());
        tokens.extend(self.mutation_tokens());
        tokens.extend(self.env.values().flat_map(Tracked::tokens));
        tokens.extend(
            self.groups
                .iter()
                .flat_map(|group| group.observed.iter().cloned()),
        );
        let population = self.task_population();
        let receiver_renames = tokens
            .iter()
            .filter(|token| !stable.contains(*token))
            .filter_map(|token| token.aged().map(|old| (token.clone(), old)))
            .collect();
        self.age_receivers(&receiver_renames);
        for token in tokens {
            if stable.contains(&token) {
                continue;
            }
            if let Some(old) = token.aged() {
                self.age_watch_token(&token, &old);
                self.age_mutation_token(&token, &old);
                let old_pending = self.pending.contains_key(&old);
                let current_pending = self.pending.contains_key(&token);
                for value in self.env.values_mut() {
                    value.age_family(
                        &token,
                        &old,
                        old_pending,
                        current_pending,
                        population.contains(&old),
                        population.contains(&token),
                    );
                }
                for object in self.objects.values_mut() {
                    for value in object.fields.values_mut() {
                        value.age_family(
                            &token,
                            &old,
                            old_pending,
                            current_pending,
                            population.contains(&old),
                            population.contains(&token),
                        );
                    }
                }
                for value in self.stores.values_mut() {
                    value.age_family(
                        &token,
                        &old,
                        old_pending,
                        current_pending,
                        population.contains(&old),
                        population.contains(&token),
                    );
                }
                for tokens in self.async_stores.values_mut() {
                    if tokens.remove(&token) {
                        tokens.insert(old.clone());
                    }
                }
                for group in &mut self.groups {
                    if group.observed.remove(&token) {
                        group.observed.insert(old.clone());
                    }
                }
                if let Some(resolved) = self.resolved.remove(&token) {
                    self.resolved.insert(old.clone(), resolved);
                }
                if let Some(obligation) = self.pending.remove(&token) {
                    self.pending
                        .entry(old)
                        .and_modify(|previous| previous.errors.extend(obligation.errors.clone()))
                        .or_insert(obligation);
                }
            }
        }
    }
    fn observe(&mut self, token: &Token) {
        if let Token::Population(inner) = token {
            self.forget_watched_producer(inner);
            self.forget_received_producer(inner);
            self.pending.remove(inner.as_ref());
            if let Token::Input(input) = inner.as_ref() {
                self.observed.insert(input.clone());
            }
            return;
        }
        if let Token::All(token) = token {
            if token.observable() {
                self.forget_received_producer(token);
                self.forget_watched_producer(token);
            }
            self.pending.remove(token.as_ref());
            if let Token::Input(parameter) = token.as_ref() {
                self.observed.insert(parameter.clone());
            }
            return;
        }
        if let Token::Iteration(at, path) = token {
            self.forget_received_producer(token);
            self.iterated.insert((*at, path.clone()));
            return;
        }
        if !token.observable() {
            return;
        }
        self.forget_watched_producer(token);
        self.forget_received_producer(token);
        self.pending.remove(token);
        if let Token::Input(parameter) = token {
            self.observed.insert(parameter.clone());
        }
    }
    fn fresh(&mut self, at: Site, slot: u32) -> Token {
        let current = Token::Local(at, slot, false);
        let old = Token::Local(at, slot, true);
        self.fresh_token(at, current, old)
    }
    fn fresh_family(&mut self, at: Site, slot: u32) -> Token {
        self.fresh_token(
            at,
            Token::Family(Box::new(Token::Local(at, slot, false))),
            Token::Family(Box::new(Token::Local(at, slot, true))),
        )
    }
    fn fresh_token(&mut self, at: Site, current: Token, old: Token) -> Token {
        if self.pending.contains_key(&current)
            || self.watch_tokens().contains(&current)
            || self.has_receiver_token(&current)
            || self.mutation_tokens().contains(&current)
            || self.object_tokens().contains(&current)
            || self
                .env
                .values()
                .any(|value| value.tokens().contains(&current))
            || self
                .groups
                .iter()
                .any(|group| group.observed.contains(&current))
        {
            let population = self.task_population();
            self.age_watch_token(&current, &old);
            self.age_receiver_token(&current, &old);
            self.age_mutation_token(&current, &old);
            if let Some(obligation) = self.pending.get(&current) {
                // Reusing a finite identity must never turn two live tasks
                // into one observation. Loop backedges normally age these
                // beforehand; an unresolved collision retains a requirement.
                let requirement = Requirement {
                    kind: RequirementKind::Observation,
                    origin: obligation.origin,
                    exit: at,
                    errors: language::restricted(&obligation.errors, &self.guards),
                };
                if !self.requirements.contains(&requirement) {
                    self.requirements.push(requirement);
                }
            }
            for value in self.env.values_mut() {
                value.age_family(
                    &current,
                    &old,
                    self.pending.contains_key(&old),
                    self.pending.contains_key(&current),
                    population.contains(&old),
                    population.contains(&current),
                );
            }
            for object in self.objects.values_mut() {
                for value in object.fields.values_mut() {
                    value.age_family(
                        &current,
                        &old,
                        self.pending.contains_key(&old),
                        self.pending.contains_key(&current),
                        population.contains(&old),
                        population.contains(&current),
                    );
                }
            }
            for value in self.stores.values_mut() {
                value.age_family(
                    &current,
                    &old,
                    self.pending.contains_key(&old),
                    self.pending.contains_key(&current),
                    population.contains(&old),
                    population.contains(&current),
                );
            }
            for tokens in self.async_stores.values_mut() {
                if tokens.remove(&current) {
                    tokens.insert(old.clone());
                }
            }
            for group in &mut self.groups {
                if group.observed.remove(&current) {
                    group.observed.insert(old.clone());
                }
            }
            if let Some(resolved) = self.resolved.remove(&current) {
                self.resolved.insert(old.clone(), resolved);
            }
            if let Some(obligation) = self.pending.remove(&current) {
                self.pending
                    .entry(old)
                    .and_modify(|old| old.errors.extend(obligation.errors.clone()))
                    .or_insert(obligation);
            }
        }
        current
    }
    fn condition(&self, guards: &language::Guards) -> Option<Self> {
        let mut next = self.clone();
        next.guards = language::merge_guards(&self.guards, guards)?;
        Some(next)
    }
    fn join(&mut self, other: &Self) {
        self.restrictions.extend(other.restrictions.clone());
        let left_population = self.task_population();
        let right_population = other.task_population();
        let left_pending = self.pending.keys().cloned().collect();
        let right_pending = other.pending.keys().cloned().collect();
        for (field, tokens) in &other.async_stores {
            self.async_stores
                .entry(field.clone())
                .or_default()
                .extend(tokens.clone());
        }
        self.stores.retain(|field, value| {
            if let Some(right) = other.stores.get(field) {
                value.join_families(
                    right.clone(),
                    &left_pending,
                    &right_pending,
                    &left_population,
                    &right_population,
                );
                true
            } else {
                false
            }
        });
        self.join_objects(other);
        self.field_watch_blocked |= other.field_watch_blocked;
        self.join_watches(other);
        self.join_receivers(other);
        self.join_mutations(other);
        self.map_appends.retain(|at| other.map_appends.contains(at));
        self.iterated.retain(|item| other.iterated.contains(item));
        self.iteration_variants.retain(|key, value| {
            if let Some(variants) = other.iteration_variants.get(key) {
                value.extend(variants.iter().cloned());
                true
            } else {
                false
            }
        });
        self.resolved.retain(|token, selected| {
            if let Some(other) = other.resolved.get(token) {
                selected.extend(other);
                true
            } else {
                false
            }
        });
        for (name, value) in &other.env {
            if self.env.get(name) == Some(value) {
                continue;
            }
            if let Some(previous) = self.env.get_mut(name) {
                previous.join_families(
                    value.clone(),
                    &left_pending,
                    &right_pending,
                    &left_population,
                    &right_population,
                );
            } else {
                self.env.insert(name.clone(), value.clone());
            }
        }
        for (token, obligation) in &other.pending {
            self.pending
                .entry(token.clone())
                .and_modify(|previous| previous.errors.extend(obligation.errors.clone()))
                .or_insert_with(|| obligation.clone());
        }
        self.observed
            .retain(|parameter| other.observed.contains(parameter));
        for requirement in &other.requirements {
            if !self.requirements.contains(requirement) {
                self.requirements.push(requirement.clone());
            }
        }
        self.guards
            .retain(|input, guard| other.guards.get(input) == Some(guard));
        for (left, right) in self.groups.iter_mut().zip(&other.groups) {
            left.errors.extend(right.errors.clone());
            left.observed.retain(|token| right.observed.contains(token));
        }
    }
}
