use super::*;
use expressions::Evaluation;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Slot {
    Future(u32),
    Object(u32),
}
#[derive(Default)]
struct Slots {
    values: BTreeMap<Slot, Token>,
    aged_objects: BTreeMap<Token, Token>,
    asynchronous: bool,
}
impl Slots {
    fn entry(&mut self, slot: Slot) -> std::collections::btree_map::Entry<'_, Slot, Token> {
        self.values.entry(slot)
    }
    fn arguments(&self, values: &[Tracked]) -> Vec<Tracked> {
        values
            .iter()
            .map(|value| {
                let mut value = value.clone();
                for (current, old) in &self.aged_objects {
                    value.rename_object(current, old);
                }
                value
            })
            .collect()
    }
}

impl Engine<'_> {
    #[allow(clippy::too_many_arguments)]
    fn transfer_receivers(
        &self,
        exit: &Exit,
        name: &str,
        values: &[Tracked],
        effects: &[Value],
        initial: &State,
        state: &mut State,
        at: Site,
        offset: u32,
        slots: &mut Slots,
    ) {
        let previous = slots.asynchronous;
        // These are captured immutable values, not new reads of mutable fields.
        slots.asynchronous = false;
        let controls = self.instantiate_tokens(
            &exit.receiver_controls,
            name,
            values,
            state,
            at,
            offset,
            slots,
        );
        for token in controls {
            state.cancel_receivers(Some(&token));
            state.receiver_controls.insert(token);
        }
        if exit.receiver_scope_control {
            state.cancel_receivers(None);
            state.receiver_scope_control = true;
        }
        for (owner, receiver) in &exit.receivers {
            let owners = self.instantiate_tokens(
                &BTreeSet::from([owner.clone()]),
                name,
                values,
                state,
                at,
                offset,
                slots,
            );
            let mut producers = BTreeMap::new();
            for (producer, receipt) in &receiver.producers {
                let mut tokens = self.instantiate_tokens(
                    &BTreeSet::from([producer.clone()]),
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                );
                tokens.extend(self.instantiate_completion_tokens(
                    &BTreeSet::from([producer.clone()]),
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                ));
                for token in tokens {
                    let token = receivers::canonical(&token);
                    let receipt = receipt
                        .as_ref()
                        .map(|receipt| Obligation {
                            origin: at,
                            errors: substitution::language(
                                &receipt.errors,
                                name,
                                effects,
                                self.bound,
                            ),
                        })
                        .or_else(|| initial.pending.get(&token).cloned())
                        .or_else(|| {
                            initial
                                .receivers
                                .values()
                                .find_map(|receiver| receiver.producers.get(&token))
                                .cloned()
                                .flatten()
                        });
                    producers.insert(token, receipt);
                }
            }
            let completes = self.instantiate_completion_tokens(
                &receiver.completes,
                name,
                values,
                state,
                at,
                offset,
                slots,
            );
            let parents =
                self.instantiate_tokens(&receiver.parents, name, values, state, at, offset, slots);
            for owner in owners {
                let receiver = receivers::Receiver {
                    origin: receiver.origin,
                    producers: producers.clone(),
                    completes: completes.clone(),
                    covers: if previous {
                        BTreeSet::new()
                    } else {
                        self.instantiate_completion_tokens(
                            &receiver.covers,
                            name,
                            values,
                            state,
                            at,
                            offset,
                            slots,
                        )
                        .iter()
                        .map(receivers::canonical)
                        .collect()
                    },
                    parents: parents.clone(),
                    valid: receiver.valid,
                    active: receiver.active,
                };
                state
                    .receivers
                    .entry(owner)
                    .and_modify(|prior| prior.merge(receiver.clone()))
                    .or_insert(receiver);
            }
        }
        slots.asynchronous = previous;
    }
    #[allow(clippy::too_many_arguments)]
    fn object_identity(
        &self,
        identity: &Token,
        name: &str,
        values: &[Tracked],
        state: &mut State,
        at: Site,
        slots: &mut Slots,
    ) -> Token {
        if let Token::Input(input) = identity
            && input.owner == name
        {
            return self
                .mutation_identity(&self.resolve_parameter(
                    input,
                    values,
                    Value::Plain,
                    false,
                    state,
                ))
                .unwrap_or_else(|| identity.clone());
        }
        let slot = match identity {
            Token::Output(slot) | Token::OutputFamily(slot) => *slot,
            _ => return identity.clone(),
        };
        if !slots.values.contains_key(&Slot::Object(slot)) {
            let (mut value, aged) = state.fresh_object(at, slot);
            if aged {
                slots
                    .aged_objects
                    .insert(value.clone(), Token::Local(at, slot, true));
            }
            if matches!(identity, Token::OutputFamily(_)) {
                value = Token::Family(Box::new(value));
            }
            slots.values.insert(Slot::Object(slot), value);
        }
        slots.values[&Slot::Object(slot)].clone()
    }
    pub fn call(
        &self,
        expression: &Expression,
        target: &[String],
        arguments: &Arguments,
        state: State,
        spawn: bool,
    ) -> Vec<Evaluation> {
        let indirect = self.checked.indirect_calls.get(&expression.range);
        let callee = if indirect.is_some() && state.env.contains_key(&target[0]) {
            self.expression(
                &Expression {
                    kind: ExpressionKind::Name(target.to_vec()),
                    range: expression.range,
                },
                state,
            )
        } else {
            vec![Evaluation::pure(state, Tracked::data(Value::Plain))]
        };
        let mut result = Vec::new();
        for callee in callee {
            let function = match callee.result {
                Ok(value) => value,
                Err(_) => {
                    result.push(callee);
                    continue;
                }
            };
            let inputs: Vec<_> = match arguments {
                Arguments::Positional(values) => values.iter().collect(),
                Arguments::Named(values) => values.iter().map(|(_, value, _)| value).collect(),
            };
            let (normal, errors) = self.sequence(inputs.into_iter(), callee.state);
            result.extend(errors);
            for (state, values) in normal {
                let targets = indirect.cloned().unwrap_or_else(|| vec![target.join(".")]);
                if targets.is_empty() {
                    result.extend(self.unknown_call(
                        expression,
                        target,
                        arguments,
                        state,
                        &values,
                        &target.join("."),
                    ));
                    continue;
                }
                for name in targets {
                    let conditions = if indirect.is_some() {
                        function.effect().target_guards(&name)
                    } else {
                        vec![BTreeMap::new()]
                    };
                    for guards in conditions {
                        let Some(state) = state.condition(&guards) else {
                            continue;
                        };
                        if let Some(contract) = self.contracts.get(&name).filter(|_| {
                            super::super::operations::scalar_constructor(
                                self.checked,
                                expression,
                                target,
                            )
                            .is_none()
                        }) {
                            let asynchronous = spawn
                                || self
                                    .checked
                                    .functions
                                    .iter()
                                    .find(|function| function.declaration.name == name)
                                    .is_some_and(|function| function.declaration.is_async);
                            if !asynchronous
                                && !self.stack.contains(&name)
                                && let Some(function) = self
                                    .checked
                                    .functions
                                    .iter()
                                    .find(|function| function.declaration.name == name)
                                && (values.iter().any(Tracked::contains_iteration)
                                    || contract.disrupts_observers
                                    || (!self.can_use_data_contract(&name)
                                        && function
                                            .parameter_types
                                            .iter()
                                            .chain(std::iter::once(&function.result_type))
                                            .any(|ty| {
                                                self.contains_mutable_class(
                                                    ty,
                                                    &mut BTreeSet::new(),
                                                )
                                            })))
                            {
                                result.extend(
                                    self.inline_synchronous(expression, function, &values, state),
                                );
                                continue;
                            }
                            result.extend(self.invoke_contract(
                                expression,
                                &name,
                                contract,
                                &values,
                                state,
                                asynchronous,
                            ));
                        } else {
                            result.extend(self.unknown_call(
                                expression, target, arguments, state, &values, &name,
                            ));
                        }
                    }
                }
            }
        }
        if spawn {
            for evaluated in &mut result {
                if let Ok(value) = &evaluated.result {
                    let errors = value.effect().future_errors(self.bound);
                    if !errors.is_empty() {
                        let requirement = Requirement {
                            kind: RequirementKind::Worker,
                            origin: self.at(expression.range),
                            exit: self.at(expression.range),
                            errors,
                        };
                        if !evaluated.state.requirements.contains(&requirement) {
                            evaluated.state.requirements.push(requirement);
                        }
                    }
                }
            }
        }
        result
    }
    fn invoke_contract(
        &self,
        expression: &Expression,
        name: &str,
        contract: &Contract,
        values: &[Tracked],
        state: State,
        asynchronous: bool,
    ) -> Vec<Evaluation> {
        let mut state = state;
        if contract.disrupts_observers {
            self.disrupts_observers.set(true);
            state.invalidate_watches(None);
            state.field_watch_blocked = true;
        }
        let mutation_targets = if asynchronous && contract.mutates {
            self.mutation_targets(contract, values, &mut state)
        } else {
            None
        };
        if contract.mutates {
            self.mutates.set(true);
            let pending = asynchronous || contract.mutates_async;
            self.mutates_async.set(self.mutates_async.get() || pending);
            if let Some(targets) = &mutation_targets {
                for object in targets {
                    state.invalidate_watches(Some(object));
                }
            } else if !contract.exports_residual_mutations || !contract.mutates_async {
                self.havoc_objects(&mut state, pending);
            }
        }
        let initial = state.clone();
        let effects: Vec<_> = values.iter().map(Tracked::effect).collect();
        if name == self.current {
            self.recursive_calls.set(true);
            if !self.structurally_smaller(values) {
                self.non_decreasing.set(true);
            }
        }
        let mut result = Vec::new();
        let mut field_candidates: BTreeMap<Token, BTreeSet<Token>> = BTreeMap::new();
        let mut stable_common: Option<BTreeSet<Token>> = None;
        let mut direct_common: Option<BTreeSet<Token>> = None;
        let mut covers_common: Option<BTreeSet<Token>> = None;
        let mut observed_common: Option<BTreeSet<Token>> = None;
        if contract.exits.is_empty() {
            let recursive = name == self.current;
            let held = if recursive {
                values
                    .iter()
                    .flat_map(Tracked::tokens)
                    .filter(|token| matches!(token, Token::Input(_)))
                    .collect()
            } else {
                BTreeSet::new()
            };
            if !asynchronous {
                return vec![Evaluation {
                    state,
                    result: Err(if recursive {
                        Stop::Recurse(held)
                    } else {
                        Stop::Diverges
                    }),
                }];
            }
            let mut state = state;
            let token = state.fresh(self.at(expression.range), 0);
            return vec![Evaluation::pure(
                state,
                Tracked::Future {
                    token,
                    result: FutureResult::Pending(held),
                    pending: false,
                },
            )];
        }
        for exit in &contract.exits {
            let conditions = Language::from([language::Word {
                prefix: Vec::new(),
                longer: false,
                guards: exit.guards.clone(),
            }]);
            for condition in substitution::language(&conditions, name, &effects, self.bound) {
                let Some(mut state) = state.condition(&condition.guards) else {
                    continue;
                };
                self.transfer_restrictions(&exit.restrictions, name, &effects, &mut state);
                let mut stable_tokens = BTreeSet::new();
                let mut direct_tokens = BTreeSet::new();
                let mut cover_tokens = BTreeSet::new();
                let mut observed_tokens = BTreeSet::new();
                for input in &exit.observed {
                    let field = asynchronous && !self.stable_projection(input);
                    let dependencies = if field {
                        if !self.allow_field_watch || state.field_watch_blocked || contract.mutates
                        {
                            continue;
                        }
                        let Some(objects) = self.observer_objects(input, values, &state) else {
                            continue;
                        };
                        Some(objects)
                    } else {
                        None
                    };
                    let value = self.resolve_parameter(input, values, Value::Plain, true, &state);
                    if !field {
                        cover_tokens.extend(self.completion_tokens(&value));
                    }
                    if exit.direct_observed.contains(input) {
                        direct_tokens.extend(self.completion_tokens(&value));
                    }
                    // A projection can select one of several tasks. Only an
                    // identity present in every alternative is discharged.
                    for token in self.observation_tokens(&value) {
                        if let Some(objects) = &dependencies {
                            if !token.observable()
                                || matches!(token, Token::All(_) | Token::Iteration(..))
                            {
                                continue;
                            }
                            field_candidates
                                .entry(token.clone())
                                .or_default()
                                .extend(objects.clone());
                        } else {
                            stable_tokens.insert(token.clone());
                        }
                        observed_tokens.insert(token.clone());
                        if asynchronous || !exit.direct_observed.contains(input) {
                            state.provisional_observe(&token);
                        } else {
                            state.observe(&token);
                        }
                    }
                }
                if let Some(common) = &mut stable_common {
                    common.retain(|token| stable_tokens.contains(token));
                } else {
                    stable_common = Some(stable_tokens);
                }
                if let Some(common) = &mut observed_common {
                    common.retain(|token| observed_tokens.contains(token));
                } else {
                    observed_common = Some(observed_tokens);
                }
                if let Some(common) = &mut direct_common {
                    common.retain(|token| direct_tokens.contains(token));
                } else {
                    direct_common = Some(direct_tokens);
                }
                if let Some(common) = &mut covers_common {
                    common.retain(|token| cover_tokens.contains(token));
                } else {
                    covers_common = Some(cover_tokens);
                }
                for requirement in &exit.requirements {
                    let requirement = Requirement {
                        kind: requirement.kind,
                        origin: requirement.origin,
                        exit: self.at(expression.range),
                        errors: substitution::language(
                            &requirement.errors,
                            name,
                            &effects,
                            self.bound,
                        ),
                    };
                    if !requirement.errors.is_empty() && !state.requirements.contains(&requirement)
                    {
                        state.requirements.push(requirement);
                    }
                }
                // Recursive/async mutable calls stay conservative until their
                // write ordering is known. They still create every obligation
                // retained in an externally stored value.
                let mut slots = Slots {
                    asynchronous,
                    ..Slots::default()
                };
                self.transfer_receivers(
                    exit,
                    name,
                    values,
                    &effects,
                    &initial,
                    &mut state,
                    self.at(expression.range),
                    u32::from(asynchronous),
                    &mut slots,
                );
                let mut saved_tokens = BTreeSet::new();
                for (token, obligation) in &exit.saved {
                    let errors =
                        substitution::language(&obligation.errors, name, &effects, self.bound);
                    if !errors.is_empty() {
                        for token in self.instantiate_tokens(
                            &BTreeSet::from([token.clone()]),
                            name,
                            values,
                            &mut state,
                            self.at(expression.range),
                            u32::from(asynchronous),
                            &mut slots,
                        ) {
                            saved_tokens.insert(token.clone());
                            state
                                .pending
                                .entry(token)
                                .and_modify(|previous| previous.errors.extend(errors.clone()))
                                .or_insert(Obligation {
                                    origin: self.at(expression.range),
                                    errors: errors.clone(),
                                });
                        }
                    }
                }
                let mut completion =
                    mutation_targets
                        .as_ref()
                        .map(|targets| mutations::Completion {
                            objects: targets
                                .iter()
                                .filter_map(|identity| {
                                    initial
                                        .objects
                                        .get(identity)
                                        .cloned()
                                        .map(|object| (identity.clone(), object))
                                })
                                .collect(),
                            observed: BTreeSet::new(),
                            active: true,
                            tainted: BTreeSet::new(),
                        });
                let previous = slots.asynchronous;
                if completion.is_some() {
                    slots.asynchronous = false;
                }
                for (field, value) in &exit.stores {
                    let value = self.instantiate(
                        value,
                        name,
                        values,
                        &effects,
                        &mut state,
                        self.at(expression.range),
                        u32::from(asynchronous),
                        &mut slots,
                    );
                    let mut base = field.clone();
                    if let Some(last) = base.path.pop()
                        && let Tracked::Input { mut parameter, .. } =
                            self.resolve_parameter(&base, values, Value::Plain, false, &initial)
                    {
                        parameter.path.push(last);
                        let entry = state.async_stores.entry(parameter).or_default();
                        entry.extend(saved_tokens.clone());
                        entry.extend(value.tokens());
                    }
                    if let Some(completion) = &mut completion {
                        let mut base = field.clone();
                        if let Some(Access::Field(_, name)) = base.path.pop()
                            && let Some(identity) = self.mutation_identity(&self.resolve_parameter(
                                &base,
                                values,
                                Value::Plain,
                                false,
                                &initial,
                            ))
                            && let Some(object) = completion.objects.get_mut(&identity)
                        {
                            object.fields.insert(name, value);
                        }
                    }
                }
                for (field, tokens) in &exit.async_stores {
                    let tokens = self.instantiate_tokens(
                        tokens,
                        name,
                        values,
                        &mut state,
                        self.at(expression.range),
                        u32::from(asynchronous),
                        &mut slots,
                    );
                    let mut base = field.clone();
                    if let Some(last) = base.path.pop()
                        && let Tracked::Input { mut parameter, .. } =
                            self.resolve_parameter(&base, values, Value::Plain, false, &initial)
                    {
                        parameter.path.push(last);
                        state
                            .async_stores
                            .entry(parameter)
                            .or_default()
                            .extend(tokens);
                    }
                }
                slots.asynchronous = previous;
                if let Some(completion) = &mut completion {
                    completion.forget_nested_completions();
                    for input in &exit.observed {
                        if !self.stable_projection(input)
                            && self.observer_objects(input, values, &initial).is_some()
                        {
                            completion.observed.extend(self.observation_tokens(
                                &self.resolve_parameter(
                                    input,
                                    values,
                                    Value::Plain,
                                    true,
                                    &initial,
                                ),
                            ));
                        }
                    }
                }
                let first_result = result.len();
                match &exit.result {
                    Ok(value) => {
                        let value = self.instantiate(
                            value,
                            name,
                            values,
                            &effects,
                            &mut state,
                            self.at(expression.range),
                            u32::from(asynchronous),
                            &mut slots,
                        );
                        if asynchronous {
                            let token = state.fresh(self.at(expression.range), 0);
                            result.push(Evaluation::pure(
                                state,
                                Tracked::Future {
                                    token,
                                    result: FutureResult::Success(Box::new(value)),
                                    pending: false,
                                },
                            ));
                        } else {
                            result.push(Evaluation::pure(state, value));
                        }
                    }
                    Err(Stop::Diverges | Stop::Panic) => {
                        let panic = matches!(exit.result, Err(Stop::Panic));
                        if asynchronous {
                            let token = state.fresh(self.at(expression.range), 0);
                            result.push(Evaluation::pure(
                                state,
                                Tracked::Future {
                                    token,
                                    result: if panic {
                                        FutureResult::Panic
                                    } else {
                                        FutureResult::Pending(BTreeSet::new())
                                    },
                                    pending: false,
                                },
                            ));
                        } else {
                            result.push(Evaluation {
                                state,
                                result: Err(if panic { Stop::Panic } else { Stop::Diverges }),
                            });
                        }
                    }
                    Err(Stop::Error(word, _)) => {
                        for word in substitution::language(
                            &Language::from([word.clone()]),
                            name,
                            &effects,
                            self.bound,
                        ) {
                            let Some(mut state) = state.condition(&word.guards) else {
                                continue;
                            };
                            if asynchronous {
                                let token = state.fresh(self.at(expression.range), 0);
                                state.pending.insert(
                                    token.clone(),
                                    Obligation {
                                        origin: self.at(expression.range),
                                        errors: Language::from([word.clone()]),
                                    },
                                );
                                result.push(Evaluation::pure(
                                    state,
                                    Tracked::Future {
                                        token,
                                        result: FutureResult::Failure(word),
                                        pending: true,
                                    },
                                ));
                            } else {
                                result.push(Evaluation::failed(state, word, None));
                            }
                        }
                    }
                    Err(Stop::Recurse(held)) => {
                        let (held, observed) = self.recursive_receivers(
                            exit,
                            contract,
                            held,
                            name,
                            values,
                            &effects,
                            &state,
                            asynchronous,
                        );
                        if asynchronous {
                            for token in observed {
                                state.observe(&token);
                            }
                            let token = state.fresh(self.at(expression.range), 0);
                            result.push(Evaluation::pure(
                                state,
                                Tracked::Future {
                                    token,
                                    result: FutureResult::Pending(held),
                                    pending: false,
                                },
                            ));
                        } else {
                            result.push(Evaluation {
                                state,
                                result: Err(Stop::Recurse(held)),
                            });
                        }
                    }
                }
                if let Some(completion) = completion {
                    for evaluated in &mut result[first_result..] {
                        if let Ok(Tracked::Future { result, .. }) = &mut evaluated.result {
                            let old =
                                std::mem::replace(result, FutureResult::Pending(BTreeSet::new()));
                            *result = FutureResult::Cases(vec![FutureCase {
                                guards: BTreeMap::new(),
                                result: old,
                                observed: BTreeSet::new(),
                                created: BTreeMap::new(),
                                completion: Some(completion.clone()),
                            }]);
                        }
                    }
                }
            }
        }
        if asynchronous {
            let mut outcomes = self.defer_outcomes(expression, &initial, result);
            let definitive = stable_common.unwrap_or_default();
            let observed = observed_common.unwrap_or_default();
            let direct = direct_common.unwrap_or_default();
            let covers = covers_common.unwrap_or_default();
            field_candidates.retain(|token, _| observed.contains(token));
            for outcome in &mut outcomes {
                if let Ok(Tracked::Future { token, .. }) = &outcome.result {
                    for (owner, receiver) in &mut outcome.state.receivers {
                        if !initial.receivers.contains_key(owner) && owner != token {
                            receiver.parents.insert(token.clone());
                        }
                    }
                    let receiver_observed = observed.union(&direct).cloned().collect();
                    outcome.state.add_receiver(
                        &initial,
                        token.clone(),
                        &receiver_observed,
                        &direct,
                        &covers,
                    );
                }
                if let Some(targets) = &mutation_targets
                    && let Ok(Tracked::Future { token, .. }) = &outcome.result
                {
                    outcome.state.begin_mutation(token.clone(), targets.clone());
                    self.obscure_mutation_targets(&mut outcome.state, targets);
                }
                for token in &definitive {
                    outcome.state.provisional_observe(token);
                }
                if let Ok(Tracked::Future { token, .. }) = &outcome.result {
                    outcome.state.add_watch(
                        &initial,
                        token.clone(),
                        &field_candidates,
                        &definitive,
                        self.at(expression.range),
                    );
                }
            }
            outcomes
        } else {
            result
        }
    }
    fn defer_outcomes(
        &self,
        expression: &Expression,
        initial: &State,
        outcomes: Vec<Evaluation>,
    ) -> Vec<Evaluation> {
        if outcomes.len() < 2 {
            return outcomes;
        }
        let Some(first) = outcomes.first() else {
            return outcomes;
        };
        if outcomes.iter().any(|outcome| {
            outcome.state.env != first.state.env
                || !matches!(outcome.result, Ok(Tracked::Future { .. }))
        }) {
            return outcomes;
        }
        let mut state = initial.clone();
        state.env = first.state.env.clone();
        let mut cases = Vec::new();
        let mut common: Option<BTreeSet<Token>> = None;
        let mut errors = Language::new();
        let token = if let Ok(Tracked::Future { token, .. }) = &first.result {
            token.clone()
        } else {
            unreachable!()
        };
        for outcome in outcomes {
            state.join_objects(&outcome.state);
            state.join_receivers(&outcome.state);
            state
                .restrictions
                .extend(outcome.state.restrictions.clone());
            state.join_mutations(&outcome.state);
            for (field, tokens) in &outcome.state.async_stores {
                state
                    .async_stores
                    .entry(field.clone())
                    .or_default()
                    .extend(tokens.clone());
            }
            let Ok(Tracked::Future { result, .. }) = outcome.result else {
                unreachable!()
            };
            let mut observed: BTreeSet<_> = initial
                .pending
                .keys()
                .filter(|token| !outcome.state.pending.contains_key(*token))
                .cloned()
                .collect();
            observed.extend(
                outcome
                    .state
                    .observed
                    .difference(&initial.observed)
                    .cloned()
                    .map(Token::Input),
            );
            // A successful exhaustive traversal also proves completion of
            // NoFail receiver tasks. Preserve that stronger fact per outcome;
            // an early Error outcome must not acquire it through a wrapper.
            for input in outcome.state.direct_observed() {
                if !initial.observed.contains(&input)
                    && input
                        .path
                        .iter()
                        .any(|access| matches!(access, Access::Each(_) | Access::MapValues(_)))
                {
                    observed.insert(Token::Population(Box::new(Token::Input(input))));
                }
            }
            observed.extend(
                outcome
                    .state
                    .iterated
                    .difference(&initial.iterated)
                    .map(|(at, path)| Token::Iteration(*at, path.clone())),
            );
            if let Some(common) = &mut common {
                common.retain(|token| observed.contains(token));
            } else {
                common = Some(observed.clone());
            }
            let created: BTreeMap<_, _> = outcome
                .state
                .pending
                .iter()
                .filter(|(candidate, _)| {
                    **candidate != token && !initial.pending.contains_key(*candidate)
                })
                .map(|(token, obligation)| (token.clone(), obligation.clone()))
                .collect();
            for (created, obligation) in &created {
                state
                    .pending
                    .entry(created.clone())
                    .and_modify(|previous| previous.errors.extend(obligation.errors.clone()))
                    .or_insert_with(|| obligation.clone());
            }
            if let Some(obligation) = outcome.state.pending.get(&token) {
                errors.extend(language::restricted(
                    &obligation.errors,
                    &outcome.state.guards,
                ));
            }
            for mut requirement in outcome.state.requirements {
                requirement.errors =
                    language::restricted(&requirement.errors, &outcome.state.guards);
                if !state.requirements.contains(&requirement) {
                    state.requirements.push(requirement);
                }
            }
            let (result, completion) = match result {
                FutureResult::Cases(mut inner)
                    if inner.len() == 1
                        && inner[0].completion.is_some()
                        && inner[0].guards.is_empty()
                        && inner[0].observed.is_empty()
                        && inner[0].created.is_empty() =>
                {
                    let case = inner.pop().unwrap();
                    (case.result, case.completion)
                }
                result => (result, None),
            };
            let case = FutureCase {
                guards: outcome.state.guards,
                result,
                observed,
                created,
                completion,
            };
            if !cases.contains(&case) {
                cases.push(case);
            }
        }
        for token in common.unwrap_or_default() {
            state.provisional_observe(&token);
        }
        let pending = !errors.is_empty();
        if pending {
            state.pending.insert(
                token.clone(),
                Obligation {
                    origin: self.at(expression.range),
                    errors,
                },
            );
        }
        vec![Evaluation::pure(
            state,
            Tracked::Future {
                token,
                result: FutureResult::Cases(cases),
                pending,
            },
        )]
    }
    fn structurally_smaller(&self, values: &[Tracked]) -> bool {
        let Some(function) = self
            .checked
            .functions
            .iter()
            .find(|function| function.declaration.name == self.current)
        else {
            return false;
        };
        values.iter().enumerate().any(|(index,value)|{
            let Tracked::Input {parameter,effect,..}=value else{return false;};
            if parameter.owner!=self.current||parameter.index!=index||parameter.path.is_empty(){return false;}
            if !matches!(function.parameter_types.get(index),Some(Type::Named(name)) if effect.nominal()==Some(name.as_str())){return false;}
            parameter.path.iter().all(|access|match access {
                Access::Field(owner,field)=>self.immutable_type(owner)||self.checked.source_types.get(owner).is_some_and(|schema|schema.fields.iter().position(|(name,_)|name==field).is_some_and(|index|schema.const_fields.contains(&(index as u32)))),
                Access::Variant(owner,_,_)=>self.checked.source_types.get(owner).is_some_and(|schema|schema.kind=="enum"),
                    Access::Await(_)|Access::Element(_,_,_)|Access::Each(_)|Access::MapValues(_)=>false,
            })
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn recursive_receivers(
        &self,
        recursive: &Exit,
        contract: &Contract,
        held: &BTreeSet<Token>,
        name: &str,
        values: &[Tracked],
        effects: &[Value],
        state: &State,
        asynchronous: bool,
    ) -> (BTreeSet<Token>, BTreeSet<Token>) {
        let mut receives: Option<BTreeSet<Token>> = None;
        let mut observes: Option<BTreeSet<Token>> = None;
        for exit in &contract.exits {
            if matches!(
                exit.result,
                Err(Stop::Diverges | Stop::Panic | Stop::Recurse(_))
            ) {
                continue;
            }
            let Some(guards) = language::merge_guards(&recursive.guards, &exit.guards) else {
                continue;
            };
            if substitution::language(
                &Language::from([language::Word {
                    prefix: Vec::new(),
                    longer: false,
                    guards,
                }]),
                name,
                effects,
                self.bound,
            )
            .is_empty()
            {
                continue;
            }
            let observed: BTreeSet<_> = exit.observed.iter().cloned().map(Token::Input).collect();
            let mut received = observed.clone();
            if let Ok(value) = &exit.result {
                received.extend(value.guaranteed());
            }
            match &mut receives {
                None => receives = Some(received),
                Some(previous) => previous.retain(|token| received.contains(token)),
            }
            match &mut observes {
                None => observes = Some(observed),
                Some(previous) => previous.retain(|token| observed.contains(token)),
            }
        }
        let resolve = |accepted: BTreeSet<Token>| {
            held.intersection(&accepted)
                .flat_map(|token| {
                    if let Token::Input(input) = token {
                        if asynchronous && !self.stable_projection(input) {
                            return BTreeSet::new();
                        }
                        self.observation_tokens(&self.resolve_parameter(
                            input,
                            values,
                            Value::Plain,
                            true,
                            state,
                        ))
                    } else {
                        BTreeSet::new()
                    }
                })
                .collect()
        };
        (
            resolve(receives.unwrap_or_default()),
            resolve(observes.unwrap_or_default()),
        )
    }
    pub(super) fn observation_tokens(&self, value: &Tracked) -> BTreeSet<Token> {
        match value {
            Tracked::AllMembers { tokens, .. } => tokens
                .iter()
                .cloned()
                .map(|token| Token::All(Box::new(token)))
                .collect(),
            Tracked::Batch(values) => values
                .iter()
                .flat_map(|value| self.observation_tokens(value))
                .collect(),
            Tracked::Iteration { at, path, .. } => {
                BTreeSet::from([Token::Iteration(*at, path.clone())])
            }
            Tracked::Future { token, .. } => BTreeSet::from([token.clone()]),
            Tracked::Input {
                parameter,
                future: true,
                ..
            } => BTreeSet::from([Token::Input(parameter.clone())]),
            Tracked::Choice(values) => {
                let mut sets = values.iter().map(|value| self.observation_tokens(value));
                let mut common = sets.next().unwrap_or_default();
                for other in sets {
                    common.retain(|token| other.contains(token));
                }
                common
            }
            _ => BTreeSet::new(),
        }
    }
    pub(super) fn completion_tokens(&self, value: &Tracked) -> BTreeSet<Token> {
        match value {
            Tracked::AllMembers { population, .. } => population
                .iter()
                .cloned()
                .map(|token| Token::Population(Box::new(token)))
                .collect(),
            Tracked::Batch(values) => values
                .iter()
                .flat_map(|value| self.completion_tokens(value))
                .collect(),
            Tracked::Input {
                parameter,
                future: true,
                ..
            } if parameter
                .path
                .iter()
                .any(|access| matches!(access, Access::Each(_) | Access::MapValues(_))) =>
            {
                BTreeSet::from([Token::Population(Box::new(Token::Input(parameter.clone())))])
            }
            Tracked::Choice(values) => {
                let mut sets = values.iter().map(|value| self.completion_tokens(value));
                let mut common = sets.next().unwrap_or_default();
                for set in sets {
                    common.retain(|token| set.contains(token));
                }
                common
            }
            _ => self.observation_tokens(value),
        }
    }
    pub(super) fn resolve_parameter(
        &self,
        input: &Parameter,
        values: &[Tracked],
        fallback: Value,
        future: bool,
        state: &State,
    ) -> Tracked {
        let mut value = values
            .get(input.index)
            .cloned()
            .unwrap_or_else(|| Tracked::unknown(fallback.clone(), BTreeSet::new()));
        for access in &input.path {
            value = if let Access::Field(owner, name) = access {
                let ty = self
                    .checked
                    .source_types
                    .get(owner)
                    .and_then(|schema| schema.fields.iter().find(|(field, _)| field == name))
                    .map(|(_, ty)| ty.clone())
                    .unwrap_or(Type::Never);
                self.read_field(state, &value, owner, name, &ty)
            } else {
                value.project(access.clone(), fallback.clone(), future)
            };
        }
        value
    }
    #[allow(clippy::too_many_arguments)]
    fn instantiate(
        &self,
        value: &Tracked,
        name: &str,
        values: &[Tracked],
        effects: &[Value],
        state: &mut State,
        at: Site,
        offset: u32,
        slots: &mut Slots,
    ) -> Tracked {
        match value {
            Tracked::Input {
                parameter,
                effect,
                future,
            } if parameter.owner == name
                && slots.asynchronous
                && !self.stable_projection(parameter) =>
            {
                Tracked::unknown(
                    substitution::value(effect, name, effects, self.bound),
                    BTreeSet::new(),
                )
            }
            Tracked::Input {
                parameter,
                effect,
                future,
            } if parameter.owner == name => self.resolve_parameter(
                parameter,
                &slots.arguments(values),
                substitution::value(effect, name, effects, self.bound),
                *future,
                state,
            ),
            Tracked::Reference {
                identity,
                ty,
                snapshot,
            } => {
                let identity = self.object_identity(identity, name, values, state, at, slots);
                if let Some(fields) = snapshot {
                    let fields = fields
                        .iter()
                        .map(|(field, value)| {
                            (
                                field.clone(),
                                self.instantiate(
                                    value, name, values, effects, state, at, offset, slots,
                                ),
                            )
                        })
                        .collect();
                    state.objects.insert(
                        identity.clone(),
                        objects::Object {
                            ty: ty.clone(),
                            fields,
                        },
                    );
                }
                Tracked::Reference {
                    identity,
                    ty: ty.clone(),
                    snapshot: None,
                }
            }
            Tracked::Data { effect, key } => Tracked::Data {
                effect: substitution::value(effect, name, effects, self.bound),
                key: key.clone(),
            },
            Tracked::Future {
                token,
                result,
                pending,
            } => {
                let token = match token {
                    Token::OutputFamily(slot) => slots
                        .entry(Slot::Future(*slot))
                        .or_insert_with(|| state.fresh_family(at, slot + offset))
                        .clone(),
                    Token::Output(slot) => slots
                        .entry(Slot::Future(*slot))
                        .or_insert_with(|| state.fresh(at, slot + offset))
                        .clone(),
                    token => token.clone(),
                };
                let results = self
                    .instantiate_outcome(result, name, values, effects, state, at, offset, slots);
                let mut mutation: Option<mutations::Mutation> = None;
                for result in &results {
                    if let Some(active) = result.active_completion() {
                        if let Some(previous) = &mut mutation {
                            previous.objects.extend(active.objects);
                            previous.tainted.extend(active.tainted);
                        } else {
                            mutation = Some(active);
                        }
                    }
                }
                if let Some(mutation) = mutation
                    && !state.mutations.contains_key(&token)
                {
                    state.begin_mutation(token.clone(), mutation.objects.clone());
                    state
                        .mutations
                        .get_mut(&token)
                        .unwrap()
                        .tainted
                        .extend(mutation.tainted);
                    self.obscure_mutation_targets(state, &mutation.objects);
                }
                let alternatives: Vec<_> = results
                    .into_iter()
                    .map(|result| Tracked::Future {
                        token: token.clone(),
                        result,
                        pending: *pending,
                    })
                    .collect();
                if *pending {
                    let errors = alternatives
                        .iter()
                        .flat_map(|value| value.effect().future_errors(self.bound))
                        .collect::<Language>();
                    if !errors.is_empty() {
                        state
                            .pending
                            .insert(token.clone(), Obligation { origin: at, errors });
                    }
                }
                choice(alternatives, Value::Bottom)
            }
            Tracked::Members(members) => {
                let element = substitution::value(&members.element, name, effects, self.bound);
                let member_tokens = self.instantiate_tokens(
                    &members.members,
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                );
                let complete = self.instantiate_tokens(
                    &members.complete,
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                );
                let pending = self.instantiate_tokens(
                    &members.pending,
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                );
                let population = self.instantiate_tokens(
                    &members.population,
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                );
                let population_complete = self.instantiate_tokens(
                    &members.population_complete,
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                );
                let errors = element.future_errors(self.bound);
                for token in &pending {
                    if !errors.is_empty() {
                        state.pending.insert(
                            token.clone(),
                            Obligation {
                                origin: at,
                                errors: errors.clone(),
                            },
                        );
                    }
                }
                Tracked::Members(FutureMembers {
                    map_origin: members.map_origin,
                    element,
                    members: member_tokens,
                    complete,
                    pending,
                    population,
                    population_complete,
                })
            }
            Tracked::Record { ty, fields } => Tracked::Record {
                ty: ty.clone(),
                fields: fields
                    .iter()
                    .map(|(field, value)| {
                        (
                            field.clone(),
                            self.instantiate(
                                value, name, values, effects, state, at, offset, slots,
                            ),
                        )
                    })
                    .collect(),
            },
            Tracked::Variant {
                ty,
                name: variant,
                fields,
            } => Tracked::Variant {
                ty: ty.clone(),
                name: variant.clone(),
                fields: fields
                    .iter()
                    .map(|value| {
                        self.instantiate(value, name, values, effects, state, at, offset, slots)
                    })
                    .collect(),
            },
            Tracked::List(values_inner) => Tracked::List(
                values_inner
                    .iter()
                    .map(|value| {
                        self.instantiate(value, name, values, effects, state, at, offset, slots)
                    })
                    .collect(),
            ),
            Tracked::Map(entries) => Tracked::Map(
                entries
                    .iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            self.instantiate(
                                value, name, values, effects, state, at, offset, slots,
                            ),
                        )
                    })
                    .collect(),
            ),
            Tracked::Choice(alternatives) => choice(
                alternatives
                    .iter()
                    .map(|value| {
                        self.instantiate(value, name, values, effects, state, at, offset, slots)
                    })
                    .collect(),
                Value::Plain,
            ),
            Tracked::Unknown { effect, held } => Tracked::unknown(
                substitution::value(effect, name, effects, self.bound),
                held.iter()
                    .map(|token| match token {
                        Token::OutputFamily(slot) => slots
                            .entry(Slot::Future(*slot))
                            .or_insert_with(|| state.fresh_family(at, slot + offset))
                            .clone(),
                        Token::Output(slot) => slots
                            .entry(Slot::Future(*slot))
                            .or_insert_with(|| state.fresh(at, slot + offset))
                            .clone(),
                        token => token.clone(),
                    })
                    .collect(),
            ),
            value => value.clone(),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn instantiate_outcome(
        &self,
        result: &FutureResult,
        name: &str,
        values: &[Tracked],
        effects: &[Value],
        state: &mut State,
        at: Site,
        offset: u32,
        slots: &mut Slots,
    ) -> Vec<FutureResult> {
        match result {
            FutureResult::Panic => vec![FutureResult::Panic],
            FutureResult::Success(value) => vec![FutureResult::Success(Box::new(
                self.instantiate(value, name, values, effects, state, at, offset, slots),
            ))],
            FutureResult::Failure(word) => {
                substitution::language(&Language::from([word.clone()]), name, effects, self.bound)
                    .into_iter()
                    .map(FutureResult::Failure)
                    .collect()
            }
            FutureResult::Pending(held) => vec![FutureResult::Pending(
                self.instantiate_tokens(held, name, values, state, at, offset, slots),
            )],
            FutureResult::Cases(cases) => {
                let mut next = Vec::new();
                for case in cases {
                    let conditions = substitution::language(
                        &Language::from([language::Word {
                            prefix: Vec::new(),
                            longer: false,
                            guards: case.guards.clone(),
                        }]),
                        name,
                        effects,
                        self.bound,
                    );
                    if conditions.is_empty() {
                        continue;
                    }
                    let results = self.instantiate_outcome(
                        &case.result,
                        name,
                        values,
                        effects,
                        state,
                        at,
                        offset,
                        slots,
                    );
                    let observed = self.instantiate_tokens(
                        &case.observed,
                        name,
                        values,
                        state,
                        at,
                        offset,
                        slots,
                    );
                    let mut created = BTreeMap::new();
                    for (token, obligation) in &case.created {
                        for token in self.instantiate_tokens(
                            &BTreeSet::from([token.clone()]),
                            name,
                            values,
                            state,
                            at,
                            offset,
                            slots,
                        ) {
                            created.insert(
                                token,
                                Obligation {
                                    origin: at,
                                    errors: substitution::language(
                                        &obligation.errors,
                                        name,
                                        effects,
                                        self.bound,
                                    ),
                                },
                            );
                        }
                    }
                    for condition in conditions {
                        for result in &results {
                            let completion = case.completion.as_ref().map(|completion| {
                                let mut objects = BTreeMap::new();
                                for (identity, object) in &completion.objects {
                                    let identity = self
                                        .object_identity(identity, name, values, state, at, slots);
                                    let fields = object
                                        .fields
                                        .iter()
                                        .map(|(field, value)| {
                                            (
                                                field.clone(),
                                                self.instantiate(
                                                    value, name, values, effects, state, at,
                                                    offset, slots,
                                                ),
                                            )
                                        })
                                        .collect();
                                    objects.insert(
                                        identity,
                                        objects::Object {
                                            ty: object.ty.clone(),
                                            fields,
                                        },
                                    );
                                }
                                let observed = self.instantiate_tokens(
                                    &completion.observed,
                                    name,
                                    values,
                                    state,
                                    at,
                                    offset,
                                    slots,
                                );
                                let tainted = completion
                                    .tainted
                                    .iter()
                                    .map(|identity| {
                                        self.object_identity(
                                            identity, name, values, state, at, slots,
                                        )
                                    })
                                    .collect();
                                mutations::Completion {
                                    objects,
                                    observed,
                                    active: completion.active,
                                    tainted,
                                }
                            });
                            next.push(FutureCase {
                                guards: condition.guards.clone(),
                                result: result.clone(),
                                observed: observed.clone(),
                                created: created.clone(),
                                completion,
                            });
                        }
                    }
                }
                vec![FutureResult::Cases(next)]
            }
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn instantiate_completion_tokens(
        &self,
        tokens: &BTreeSet<Token>,
        name: &str,
        values: &[Tracked],
        state: &mut State,
        at: Site,
        offset: u32,
        slots: &mut Slots,
    ) -> BTreeSet<Token> {
        tokens
            .iter()
            .flat_map(|token| match token {
                Token::Input(input) if input.owner == name => {
                    self.completion_tokens(&self.resolve_parameter(
                        input,
                        &slots.arguments(values),
                        Value::Plain,
                        true,
                        state,
                    ))
                }
                _ => self.instantiate_tokens(
                    &BTreeSet::from([token.clone()]),
                    name,
                    values,
                    state,
                    at,
                    offset,
                    slots,
                ),
            })
            .collect()
    }
    #[allow(clippy::too_many_arguments)]
    fn instantiate_tokens(
        &self,
        tokens: &BTreeSet<Token>,
        name: &str,
        values: &[Tracked],
        state: &mut State,
        at: Site,
        offset: u32,
        slots: &mut Slots,
    ) -> BTreeSet<Token> {
        tokens
            .iter()
            .flat_map(|token| match token {
                Token::All(inner) => self
                    .instantiate_tokens(
                        &BTreeSet::from([inner.as_ref().clone()]),
                        name,
                        values,
                        state,
                        at,
                        offset,
                        slots,
                    )
                    .iter()
                    .map(|token| Token::All(Box::new(receivers::canonical(token))))
                    .collect(),
                Token::Population(inner) => self
                    .instantiate_completion_tokens(
                        &BTreeSet::from([inner.as_ref().clone()]),
                        name,
                        values,
                        state,
                        at,
                        offset,
                        slots,
                    )
                    .iter()
                    .map(|token| Token::Population(Box::new(receivers::canonical(token))))
                    .collect(),
                Token::Input(input)
                    if input.owner == name
                        && slots.asynchronous
                        && !self.stable_projection(input) =>
                {
                    BTreeSet::new()
                }
                Token::Input(input) if input.owner == name => {
                    self.observation_tokens(&self.resolve_parameter(
                        input,
                        &slots.arguments(values),
                        Value::Plain,
                        true,
                        state,
                    ))
                }
                Token::Output(slot) => BTreeSet::from([slots
                    .entry(Slot::Future(*slot))
                    .or_insert_with(|| state.fresh(at, slot + offset))
                    .clone()]),
                Token::OutputFamily(slot) => BTreeSet::from([slots
                    .entry(Slot::Future(*slot))
                    .or_insert_with(|| state.fresh_family(at, slot + offset))
                    .clone()]),
                token => BTreeSet::from([token.clone()]),
            })
            .collect()
    }
    fn unknown_call(
        &self,
        expression: &Expression,
        target: &[String],
        arguments: &Arguments,
        state: State,
        values: &[Tracked],
        name: &str,
    ) -> Vec<Evaluation> {
        // Argument evaluation has already happened. Panic never returns and
        // bypasses catch handlers, but must still visit every enclosing finally.
        if self.checked.data_operations.get(&expression.range) == Some(&crate::DataOperation::Panic)
        {
            return vec![Evaluation {
                state,
                result: Err(Stop::Panic),
            }];
        }
        let mut state = state;
        if matches!(
            self.checked.task_controls.get(&expression.range),
            Some(
                crate::TaskControlKind::Cancel
                    | crate::TaskControlKind::Timeout
                    | crate::TaskControlKind::Shutdown
            )
        ) {
            if matches!(
                self.checked.task_controls.get(&expression.range),
                Some(crate::TaskControlKind::Shutdown)
            ) {
                state.cancel_receivers(None);
                state.receiver_scope_control = true;
            } else if let Some(Tracked::Future { token, .. }) = values.first() {
                state.cancel_receivers(Some(token));
                state.receiver_controls.insert(token.clone());
            } else if let Some(Tracked::Input {
                parameter,
                future: true,
                ..
            }) = values.first()
            {
                state.cancel_receivers(Some(&Token::Input(parameter.clone())));
                state
                    .receiver_controls
                    .insert(Token::Input(parameter.clone()));
            } else {
                state.cancel_receivers(None);
                state.receiver_scope_control = true;
            }
            self.disrupts_observers.set(true);
            state.invalidate_watches(None);
            state.field_watch_blocked = true;
        }
        // Flat native records can change without a callback. Their references
        // participate in alias/mutation analysis just like managed mutable data.
        let mutating_entry = self.checked.native_calls.contains_key(&expression.range)
            || self.checked.indirect_calls.contains_key(&expression.range)
            || self.checked.supplant_calls.contains_key(&expression.range)
            || self
                .checked
                .source_native_calls
                .get(&expression.range)
                .is_some_and(|signature| {
                    signature.parameters.iter().any(|ty| {
                        matches!(
                            ty,
                            crate::SourceAbiType::Callback { .. }
                                | crate::SourceAbiType::UserData(_)
                                | crate::SourceAbiType::Record(_)
                        )
                    })
                });
        if mutating_entry {
            let argument_values: Vec<_> = match arguments {
                Arguments::Positional(values) => values.iter().collect(),
                Arguments::Named(values) => values.iter().map(|(_, value, _)| value).collect(),
            };
            if values
                .iter()
                .any(|value| self.callback_scope_control(&value.effect()))
                || argument_values.iter().any(|argument| {
                    let ExpressionKind::Name(path) = &argument.kind else {
                        return false;
                    };
                    matches!(self.type_at(argument.range), Type::StaticHandler(_))
                        && self.contracts.get(&path.join(".")).is_some_and(|contract| {
                            contract
                                .exits
                                .iter()
                                .any(|exit| exit.receiver_scope_control)
                        })
                })
            {
                state.cancel_receivers(None);
                state.receiver_scope_control = true;
            }
            if self.checked.native_calls.contains_key(&expression.range)
                || self
                    .checked
                    .source_native_calls
                    .contains_key(&expression.range)
                || self.checked.supplant_calls.contains_key(&expression.range)
            {
                self.check_native_entries(expression, &argument_values, values, &mut state);
                for (argument, value) in argument_values.iter().zip(values) {
                    let boundary = if self.checked.native_captures.contains_key(&argument.range) {
                        retention::Boundary::NativeCapture
                    } else {
                        retention::Boundary::Native
                    };
                    self.restrict_values(
                        boundary,
                        self.at(expression.range),
                        [value.effect()],
                        &mut state,
                    );
                }
            }
            self.disrupts_observers.set(true);
            state.invalidate_watches(None);
            state.field_watch_blocked = true;
            self.mutates.set(true);
            let asynchronous = self.checked.supplant_calls.contains_key(&expression.range)
                || self.checked.indirect_calls.contains_key(&expression.range)
                || self
                    .checked
                    .source_native_calls
                    .get(&expression.range)
                    .is_some_and(|signature| signature.is_async || signature.is_supplant)
                || self.checked.native_calls.contains_key(&expression.range);
            self.havoc_objects(&mut state, asynchronous);
            self.mutates_async
                .set(self.mutates_async.get() || asynchronous);
        }
        let analyzer =
            super::super::flow::Analyzer::new(self.checked, self.effects, self.bound, false);
        let mut errors = analyzer.primitive_errors(expression, target, arguments, name);
        if let Some(scalar) =
            super::super::operations::scalar_constructor(self.checked, expression, target)
            && let Some((minimum, maximum)) = scalar.integer_bounds()
            && let Some(Key::Int(value)) = values.first().and_then(Tracked::key)
        {
            if (minimum..=maximum).contains(&i128::from(value)) {
                errors.clear();
            } else {
                return self.errors(state, &errors, None);
            }
        }
        let mut result = self.errors(state.clone(), &errors, None);
        if let Type::Future(inner) = self.type_at(expression.range) {
            let outcomes = language::unknown(self.bound)
                .into_iter()
                .map(FutureResult::Failure)
                .chain([FutureResult::Success(Box::new(Tracked::data(
                    self.typed(&inner),
                )))]);
            for outcome in outcomes {
                let mut state = state.clone();
                let token = state.fresh(self.at(expression.range), 0);
                let pending = matches!(outcome, FutureResult::Failure(_));
                if let FutureResult::Failure(word) = &outcome {
                    state.pending.insert(
                        token.clone(),
                        Obligation {
                            origin: self.at(expression.range),
                            errors: Language::from([word.clone()]),
                        },
                    );
                }
                result.push(Evaluation::pure(
                    state,
                    Tracked::Future {
                        token,
                        result: outcome,
                        pending,
                    },
                ));
            }
        } else {
            for value in self.primitive_value(expression, target, arguments, values, &mut state) {
                result.push(Evaluation::pure(state.clone(), value));
            }
        }
        if matches!(self.type_at(expression.range), Type::Future(_)) {
            let (normal, mut failed): (Vec<_>, Vec<_>) = result
                .into_iter()
                .partition(|evaluation| evaluation.result.is_ok());
            failed.extend(self.defer_outcomes(expression, &state, normal));
            failed
        } else {
            result
        }
    }
    fn primitive_value(
        &self,
        expression: &Expression,
        target: &[String],
        arguments: &Arguments,
        values: &[Tracked],
        state: &mut State,
    ) -> Vec<Tracked> {
        if let Some(operation) = self.checked.data_operations.get(&expression.range) {
            return self.collection(*operation, expression, values, state);
        }
        let ty = self.type_at(expression.range);
        if super::super::operations::scalar_constructor(self.checked, expression, target).is_some()
            && ty.is_integer()
            && let Some(key @ Key::Int(_)) = values.first().and_then(Tracked::key)
        {
            return vec![Tracked::Data {
                effect: self.typed(&ty),
                key: Some(key),
            }];
        }
        let owner = ty.to_string();
        if let Some(schema) = self.checked.source_types.get(&owner) {
            if schema.kind == "error" {
                self.restrict_values(
                    retention::Boundary::ErrorPayload,
                    self.at(expression.range),
                    values.iter().map(Tracked::effect),
                    state,
                );
                return vec![Tracked::data(self.typed(&ty))];
            }
            if schema.kind == "enum" && target.len() > 1 {
                let name = target.last().unwrap();
                if let Some((_, fields)) =
                    schema.variants.iter().find(|(variant, _)| variant == name)
                {
                    let fields = ordered(arguments, values, fields.iter().map(|(name, _)| name));
                    return vec![Tracked::Variant {
                        ty: owner,
                        name: name.clone(),
                        fields,
                    }];
                }
            }
            if target.join(".") == owner {
                let fields = ordered(
                    arguments,
                    values,
                    schema.fields.iter().map(|(name, _)| name),
                );
                return vec![if self.immutable_type(&owner) {
                    Tracked::Record {
                        ty: owner,
                        fields: schema
                            .fields
                            .iter()
                            .map(|(name, _)| name.clone())
                            .zip(fields)
                            .collect(),
                    }
                } else {
                    let (identity, aged) = state.fresh_object(self.at(expression.range), 0);
                    let mut fields: BTreeMap<_, _> = schema
                        .fields
                        .iter()
                        .map(|(name, _)| name.clone())
                        .zip(fields)
                        .collect();
                    if aged {
                        for value in fields.values_mut() {
                            value.rename_object(
                                &identity,
                                &Token::Local(self.at(expression.range), 0, true),
                            );
                        }
                    }
                    state.objects.insert(
                        identity.clone(),
                        objects::Object {
                            ty: owner.clone(),
                            fields,
                        },
                    );
                    Tracked::Reference {
                        identity,
                        ty: owner,
                        snapshot: None,
                    }
                }];
            }
        }
        vec![self.pure(expression.range)]
    }
    fn collection(
        &self,
        operation: crate::DataOperation,
        expression: &Expression,
        values: &[Tracked],
        state: &State,
    ) -> Vec<Tracked> {
        use crate::DataOperation::*;
        let Some(first) = values.first() else {
            return vec![self.pure(expression.range)];
        };
        let ty = self.type_at(expression.range).to_string();
        let some = |value| Tracked::Variant {
            ty: ty.clone(),
            name: "Some".into(),
            fields: vec![value],
        };
        let none = || Tracked::Variant {
            ty: ty.clone(),
            name: "None".into(),
            fields: vec![],
        };
        match operation {
            MapKeys => vec![Tracked::Keys(Box::new(first.clone()))],
            ListLength => {
                let name = match &expression.kind {
                    ExpressionKind::Call {
                        arguments: Arguments::Positional(arguments),
                        ..
                    }
                    | ExpressionKind::GenericCall {
                        arguments: Arguments::Positional(arguments),
                        ..
                    } => arguments.first().and_then(|argument| {
                        if let ExpressionKind::Name(path) = &argument.kind {
                            (path.len() == 1).then(|| path[0].clone())
                        } else {
                            None
                        }
                    }),
                    _ => None,
                };
                name.map_or_else(
                    || vec![self.pure(expression.range)],
                    |name| {
                        vec![Tracked::Length {
                            at: self.at(expression.range),
                            name,
                            collection: Box::new(first.clone()),
                        }]
                    },
                )
            }
            ListGet | MapGet => {
                let result = self.type_at(expression.range);
                let field = self
                    .checked
                    .source_types
                    .get(&result.to_string())
                    .and_then(|schema| schema.variants.iter().find(|(name, _)| name == "Some"))
                    .and_then(|(_, fields)| fields.first())
                    .map(|(_, ty)| ty.clone())
                    .unwrap_or(Type::Never);
                let key = values.get(1).and_then(Tracked::key);
                if operation == MapGet
                    && let Some(Tracked::Iteration { at, base, path, .. }) = values.get(1)
                    && matches!(path.as_slice(), [Access::Each(_)])
                    && let Tracked::Keys(snapshot) = base.as_ref()
                    && first.same_map_snapshot(snapshot)
                {
                    let input = self.call_input_type(expression);
                    return vec![some(Tracked::Iteration {
                        at: *at,
                        base: base.clone(),
                        path: vec![
                            Access::MapValues(input.clone()),
                            Access::Each(input.clone()),
                        ],
                        effect: first
                            .effect()
                            .project(Projection::AllElements(input), self.typed(&field)),
                    })];
                }
                if operation == ListGet
                    && let Some(Tracked::Data {
                        key: Some(Key::Iteration(at)),
                        ..
                    }) = values.get(1)
                    && matches!(first,Tracked::Iteration{at:owner,path,..} if owner==at&&path.is_empty())
                {
                    return vec![some(first.project(
                        Access::Each(self.call_input_type(expression)),
                        self.typed(&field),
                        matches!(field, Type::Future(_)),
                    ))];
                }
                if let (Tracked::List(entries), Some(Key::Int(index))) = (first, &key) {
                    return usize::try_from(*index)
                        .ok()
                        .and_then(|index| entries.get(index))
                        .map_or_else(|| vec![none()], |value| vec![some(value.clone())]);
                }
                if let (Tracked::Map(entries), Some(key)) = (first, &key) {
                    let mut result = Vec::new();
                    let mut present = false;
                    for (candidate, value) in entries.iter().rev() {
                        if candidate.as_ref().is_none_or(|candidate| candidate == key) {
                            result.push(some(value.clone()));
                        }
                        if candidate.as_ref() == Some(key) {
                            present = true;
                            break;
                        }
                    }
                    if !present {
                        result.push(none());
                    }
                    return result;
                }
                let value = first.project(
                    Access::Element(
                        self.call_input_type(expression),
                        key,
                        self.at(expression.range),
                    ),
                    self.typed(&field),
                    matches!(field, Type::Future(_)),
                );
                let mut result = value
                    .alternatives()
                    .into_iter()
                    .map(some)
                    .collect::<Vec<_>>();
                result.push(none());
                result
            }
            ListAppend => {
                if let Tracked::Members(members) = first {
                    let mut members = members.clone();
                    if members.append(&values[1]) {
                        return vec![Tracked::Members(members)];
                    }
                }
                if let Tracked::List(entries) = first {
                    let mut entries = entries.clone();
                    entries.push(values[1].clone());
                    vec![Tracked::List(entries)]
                } else {
                    vec![self.unknown_collection(expression, values)]
                }
            }
            ListSet | ListRemove => {
                if let (Tracked::List(entries), Some(Key::Int(index))) =
                    (first, values.get(1).and_then(Tracked::key))
                {
                    let Some(index) = usize::try_from(index)
                        .ok()
                        .filter(|index| *index < entries.len())
                    else {
                        return vec![none()];
                    };
                    let mut entries = entries.clone();
                    if operation == ListSet {
                        entries[index] = values[2].clone();
                    } else {
                        entries.remove(index);
                    }
                    vec![some(Tracked::List(entries))]
                } else {
                    vec![some(self.unknown_collection(expression, values)), none()]
                }
            }
            MapInsert => {
                if let Tracked::Members(members) = first
                    && members.map_origin == Some(self.at(expression.range))
                    && state.map_appends.contains(&self.at(expression.range))
                {
                    let mut members = members.clone();
                    if members.append(&values[2]) {
                        return vec![Tracked::Members(members)];
                    }
                }
                if let Tracked::Map(entries) = first {
                    let mut entries = entries.clone();
                    entries.push((values.get(1).and_then(Tracked::key), values[2].clone()));
                    vec![value::map(entries)]
                } else {
                    vec![self.unknown_collection(expression, values)]
                }
            }
            MapRemove => {
                if let (Tracked::Map(entries), Some(key)) =
                    (first, values.get(1).and_then(Tracked::key))
                    && entries.iter().all(|(key, _)| key.is_some())
                {
                    vec![Tracked::Map(
                        entries
                            .iter()
                            .filter(|(candidate, _)| candidate.as_ref() != Some(&key))
                            .cloned()
                            .collect(),
                    )]
                } else {
                    vec![self.unknown_collection(expression, values)]
                }
            }
            _ => vec![self.pure(expression.range)],
        }
    }
    fn call_input_type(&self, expression: &Expression) -> String {
        let arguments = match &expression.kind {
            ExpressionKind::Call {
                arguments: Arguments::Positional(values),
                ..
            }
            | ExpressionKind::GenericCall {
                arguments: Arguments::Positional(values),
                ..
            } => values,
            _ => return String::new(),
        };
        arguments
            .first()
            .map_or_else(String::new, |value| self.type_at(value.range).to_string())
    }
    fn unknown_collection(&self, expression: &Expression, values: &[Tracked]) -> Tracked {
        Tracked::unknown(
            self.typed(&self.type_at(expression.range)),
            values.iter().flat_map(Tracked::tokens).collect(),
        )
    }
}
fn ordered<'a>(
    arguments: &Arguments,
    values: &[Tracked],
    fields: impl Iterator<Item = &'a String>,
) -> Vec<Tracked> {
    fields
        .enumerate()
        .map(|(index, field)| {
            let index = match arguments {
                Arguments::Positional(_) => Some(index),
                Arguments::Named(arguments) => {
                    arguments.iter().position(|(name, _, _)| name == field)
                }
            };
            index
                .and_then(|index| values.get(index))
                .cloned()
                .unwrap_or_else(|| Tracked::unknown(Value::Plain, BTreeSet::new()))
        })
        .collect()
}
