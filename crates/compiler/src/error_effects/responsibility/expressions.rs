use super::*;

pub(super) struct Evaluation {
    pub state: State,
    pub result: Result<Tracked, Stop>,
}
impl Evaluation {
    pub fn pure(state: State, value: Tracked) -> Self {
        Self {
            state,
            result: Ok(value),
        }
    }
    pub fn failed(state: State, word: language::Word, token: Option<Token>) -> Self {
        Self {
            state,
            result: Err(Stop::Error(word, token)),
        }
    }
    pub fn path(self) -> Path {
        Path {
            state: self.state,
            control: match self.result {
                Ok(_) => Control::Normal,
                Err(stop) => stop.control(),
            },
        }
    }
}
impl Engine<'_> {
    fn await_outcome(
        &self,
        token: Token,
        outcome: FutureResult,
        mut state: State,
    ) -> Vec<Evaluation> {
        state.observe(&token);
        match outcome {
            FutureResult::Cases(cases) => {
                let resolved = state.resolved.get(&token).cloned();
                let created: BTreeSet<_> = cases
                    .iter()
                    .flat_map(|case| case.created.keys().cloned())
                    .collect();
                let mut result = Vec::new();
                for (index, case) in cases.into_iter().enumerate() {
                    if resolved
                        .as_ref()
                        .is_some_and(|selected| !selected.contains(&index))
                    {
                        continue;
                    }
                    let Some(mut state) = state.condition(&case.guards) else {
                        continue;
                    };
                    if resolved.is_none() {
                        state.pending.retain(|token, _| !created.contains(token));
                        state.pending.extend(case.created);
                    }
                    // Keep repeated awaits on the same outcome. The inner
                    // tasks are created once, including across Future aliases.
                    state
                        .resolved
                        .insert(token.clone(), BTreeSet::from([index]));
                    for observed in case.observed {
                        state.observe_after_watch(&token, &observed);
                    }
                    if matches!(
                        case.result,
                        FutureResult::Success(_) | FutureResult::Failure(_) | FutureResult::Panic
                    ) {
                        state.finish_mutation(&token, case.completion.as_ref());
                    }
                    result.extend(self.await_outcome(token.clone(), case.result, state));
                }
                result
            }
            FutureResult::Panic => {
                state.finish_mutation(&token, None);
                vec![Evaluation {
                    state,
                    result: Err(Stop::Panic),
                }]
            }
            FutureResult::Pending(held) => vec![Evaluation {
                state,
                result: Err(if held.is_empty() {
                    Stop::Diverges
                } else {
                    Stop::Recurse(held)
                }),
            }],
            FutureResult::Success(value) => {
                state.finish_mutation(&token, None);
                state.complete_watch(&token);
                if let Some(group) = state.groups.last_mut() {
                    group.observed.insert(token);
                }
                vec![Evaluation::pure(state, *value)]
            }
            FutureResult::Failure(word) => {
                state.finish_mutation(&token, None);
                state.complete_watch(&token);
                vec![Evaluation::failed(state, word, Some(token))]
            }
        }
    }
    pub fn expression(&self, expression: &Expression, state: State) -> Vec<Evaluation> {
        coalesce(self, self.raw_expression(expression, state))
    }
    fn raw_expression(&self, expression: &Expression, state: State) -> Vec<Evaluation> {
        match &expression.kind {
            ExpressionKind::GenericReference { .. } => {
                unreachable!("generic references are specialized before responsibility analysis")
            }
            ExpressionKind::Name(path) => {
                let mut value = if let Some(ty) =
                    self.checked.error_narrowings.get(&expression.range)
                {
                    Tracked::data(self.typed(ty))
                } else if let Some(name) = self.checked.static_references.get(&expression.range) {
                    Tracked::data(Value::Callable(BTreeSet::from([name.clone()])))
                } else {
                    state
                        .env
                        .get(&path[0])
                        .cloned()
                        .unwrap_or_else(|| self.pure(expression.range))
                };
                if value == Tracked::Unbound {
                    return self.errors(state, &language::one(Tag::Any), None);
                }
                let native_read = self.checked.native_record_reads.contains(&expression.range);
                for (index, name) in path[1..].iter().enumerate() {
                    let owner = value.effect().nominal().unwrap_or_default().to_owned();
                    let ty = self
                        .checked
                        .source_types
                        .get(&owner)
                        .and_then(|schema| schema.fields.iter().find(|(field, _)| field == name))
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or_else(|| {
                            if index + 2 == path.len() {
                                self.type_at(expression.range)
                            } else {
                                Type::Never
                            }
                        });
                    value = self.read_field(&state, &value, &owner, name, &ty);
                }
                let mut result = if native_read {
                    self.errors(state.clone(), &language::one(Tag::Any), None)
                } else {
                    Vec::new()
                };
                result.push(Evaluation::pure(state, value));
                result
            }
            ExpressionKind::Literal(literal) => {
                let key = match literal {
                    Literal::Int(value) => Some(Key::Int(*value)),
                    Literal::String(value) => Some(Key::String(value.clone())),
                    Literal::Bool(value) => Some(Key::Bool(*value)),
                    Literal::DecimalInteger(value) => {
                        value.replace('_', "").parse().ok().map(Key::Int)
                    }
                    _ => None,
                };
                let effect = match literal {
                    Literal::Bool(value) => Value::Bool(Some(*value)),
                    _ => self.typed(&self.type_at(expression.range)),
                };
                vec![Evaluation::pure(state, Tracked::Data { effect, key })]
            }
            ExpressionKind::Unit | ExpressionKind::Function(_) => {
                vec![Evaluation::pure(state, self.pure(expression.range))]
            }
            ExpressionKind::Call { target, arguments }
            | ExpressionKind::GenericCall {
                target, arguments, ..
            } => self.call(expression, target, arguments, state, false),
            ExpressionKind::Spawn { target, arguments } => {
                self.call(expression, target, arguments, state, true)
            }
            ExpressionKind::Await(operand) => {
                let mut result = Vec::new();
                for evaluated in self.expression(operand, state) {
                    let value = match evaluated.result {
                        Ok(value) => value,
                        Err(_) => {
                            result.push(evaluated);
                            continue;
                        }
                    };
                    for value in value.alternatives() {
                        let mut state = evaluated.state.clone();
                        match value {
                            Tracked::Future {
                                token,
                                result: outcome,
                                ..
                            } => {
                                result.extend(self.await_outcome(token, outcome, state));
                            }
                            value => {
                                if let Tracked::Input {
                                    parameter,
                                    future: true,
                                    ..
                                } = &value
                                {
                                    state.observe(&Token::Input(parameter.clone()));
                                }
                                if let Tracked::Iteration { at, path, .. } = &value {
                                    state.observe(&Token::Iteration(*at, path.clone()));
                                }
                                let token = if let Tracked::Input {
                                    parameter,
                                    future: true,
                                    ..
                                } = &value
                                {
                                    Some(Token::Input(parameter.clone()))
                                } else if let Tracked::Iteration { at, path, .. } = &value {
                                    Some(Token::Iteration(*at, path.clone()))
                                } else {
                                    None
                                };
                                for guards in value.effect().panic_guards() {
                                    if let Some(state) = state.condition(&guards) {
                                        result.push(Evaluation {
                                            state,
                                            result: Err(Stop::Panic),
                                        });
                                    }
                                }
                                for guards in value.effect().nonreturn_guards() {
                                    if let Some(state) = state.condition(&guards) {
                                        result.push(Evaluation {
                                            state,
                                            result: Err(Stop::Diverges),
                                        });
                                    }
                                }
                                for word in value.effect().future_errors(self.bound) {
                                    if let Some(state) = state.condition(&word.guards) {
                                        result.push(Evaluation::failed(state, word, token.clone()));
                                    }
                                }
                                for guards in value.effect().success_guards() {
                                    if let Some(mut state) = state.condition(&guards) {
                                        if let (Some(token), Some(group)) =
                                            (&token, state.groups.last_mut())
                                        {
                                            group.observed.insert(token.clone());
                                        }
                                        let inner = self.type_at(expression.range);
                                        let returned = value.project(
                                            Access::Await(self.type_at(operand.range).to_string()),
                                            self.typed(&inner),
                                            matches!(inner, Type::Future(_)),
                                        );
                                        result.push(Evaluation::pure(state, returned));
                                    }
                                }
                            }
                        }
                    }
                }
                result
            }
            ExpressionKind::Is { value, .. } => self.map_expression(value, state, |value| {
                Tracked::data(Value::Bool(
                    match (
                        &value.effect(),
                        self.checked
                            .error_tests
                            .get(&expression.range)
                            .and_then(|ty| tag(self.checked, ty)),
                    ) {
                        (Value::Error(tags), Some(expected))
                            if tags.iter().all(|tag| *tag == expected) =>
                        {
                            Some(true)
                        }
                        (Value::Error(tags), Some(expected))
                            if tags
                                .iter()
                                .all(|tag| matches!(tag, Tag::Named(_)) && *tag != expected) =>
                        {
                            Some(false)
                        }
                        _ => None,
                    },
                ))
            }),
            ExpressionKind::Unary { operator, operand } => {
                self.map_expression(operand, state, |value| {
                    let key = match (operator, value.key()) {
                        (UnaryOperator::Not, Some(Key::Bool(value))) => Some(Key::Bool(!value)),
                        (UnaryOperator::Negate, Some(Key::Int(value))) => {
                            value.checked_neg().map(Key::Int)
                        }
                        _ => None,
                    };
                    Tracked::Data {
                        effect: if let Some(Key::Bool(value)) = key {
                            Value::Bool(Some(value))
                        } else {
                            self.typed(&self.type_at(expression.range))
                        },
                        key,
                    }
                })
            }
            ExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let mut result = Vec::new();
                for first in self.expression(left, state) {
                    let a = match first.result {
                        Ok(value) => value,
                        Err(_) => {
                            result.push(first);
                            continue;
                        }
                    };
                    let short = match operator {
                        BinaryOperator::And => Some(false),
                        BinaryOperator::Or => Some(true),
                        _ => None,
                    };
                    if let Some(skip) = short {
                        for guards in a.effect().bool_guards(skip) {
                            if let Some(state) = first.state.condition(&guards) {
                                result.push(Evaluation::pure(
                                    state,
                                    Tracked::data(Value::Bool(Some(skip))),
                                ));
                            }
                        }
                    }
                    let paths = short.map_or_else(
                        || vec![BTreeMap::new()],
                        |skip| a.effect().bool_guards(!skip),
                    );
                    for guards in paths {
                        if let Some(state) = first.state.condition(&guards) {
                            for mut second in self.expression(right, state) {
                                if let Ok(b) = &second.result {
                                    second.result = Ok(self.binary(expression, *operator, &a, b));
                                }
                                result.push(second);
                            }
                        }
                    }
                }
                result
            }
            ExpressionKind::FormatString(values) | ExpressionKind::List(values) => {
                let native_display = matches!(expression.kind, ExpressionKind::FormatString(_))
                    && values.iter().any(|value| {
                        crate::sema::display_reads_native_record(
                            &self.checked.source_types,
                            &self.type_at(value.range),
                        )
                    });
                let (normal, mut result) = self.sequence(values.iter(), state);
                for (state, values) in normal {
                    if native_display {
                        result.extend(self.errors(state.clone(), &language::one(Tag::Any), None));
                    }
                    result.push(Evaluation::pure(
                        state,
                        if matches!(expression.kind, ExpressionKind::List(_)) {
                            Tracked::List(values)
                        } else {
                            self.pure(expression.range)
                        },
                    ));
                }
                result
            }
            ExpressionKind::Map(entries) => {
                let (normal, mut result) =
                    self.sequence(entries.iter().flat_map(|(key, value)| [key, value]), state);
                for (state, values) in normal {
                    result.push(Evaluation::pure(
                        state,
                        value::map(
                            values
                                .chunks_exact(2)
                                .map(|pair| (pair[0].key(), pair[1].clone())),
                        ),
                    ));
                }
                result
            }
            ExpressionKind::Record(fields) => {
                let (normal, mut result) =
                    self.sequence(fields.iter().map(|field| &field.value), state);
                for (state, values) in normal {
                    let fields = fields
                        .iter()
                        .zip(values)
                        .map(|(field, value)| (field.name.clone(), value))
                        .collect();
                    let ty = self.type_at(expression.range).to_string();
                    let value = if self.immutable_type(&ty) {
                        Tracked::Record { ty, fields }
                    } else {
                        Tracked::unknown(
                            self.typed(&self.type_at(expression.range)),
                            fields.values().flat_map(Tracked::tokens).collect(),
                        )
                    };
                    result.push(Evaluation::pure(state, value));
                }
                result
            }
            ExpressionKind::RecordUpdate { base, fields } => {
                let mut result = Vec::new();
                for evaluated in self.expression(base, state) {
                    let base = match evaluated.result {
                        Ok(value) => value,
                        Err(_) => {
                            result.push(evaluated);
                            continue;
                        }
                    };
                    let (normal, errors) =
                        self.sequence(fields.iter().map(|(_, value, _)| value), evaluated.state);
                    result.extend(errors);
                    for (state, values) in normal {
                        let mut updated = if let Tracked::Record { fields, .. } = &base {
                            fields.clone()
                        } else {
                            BTreeMap::new()
                        };
                        for ((name, _, _), value) in fields.iter().zip(values) {
                            updated.insert(name.clone(), value);
                        }
                        // An unknown base can contain responsibilities in fields
                        // not named by the update. Keep those identities opaque.
                        let value = if matches!(base, Tracked::Record { .. }) {
                            Tracked::Record {
                                ty: self.type_at(expression.range).to_string(),
                                fields: updated,
                            }
                        } else {
                            let mut held = base.tokens();
                            held.extend(updated.values().flat_map(Tracked::tokens));
                            Tracked::unknown(self.typed(&self.type_at(expression.range)), held)
                        };
                        result.push(Evaluation::pure(state, value));
                    }
                }
                result
            }
            ExpressionKind::Bind { arguments, .. } => {
                let values: Vec<_> = match arguments {
                    Arguments::Positional(values) => values.iter().collect(),
                    Arguments::Named(values) => values.iter().map(|(_, value, _)| value).collect(),
                };
                let (normal, mut result) = self.sequence(values.into_iter(), state);
                for (state, values) in normal {
                    result.push(Evaluation::pure(
                        state,
                        Tracked::unknown(
                            self.typed(&self.type_at(expression.range)),
                            values.iter().flat_map(Tracked::tokens).collect(),
                        ),
                    ));
                }
                result
            }
        }
    }
    fn binary(
        &self,
        expression: &Expression,
        operator: BinaryOperator,
        a: &Tracked,
        b: &Tracked,
    ) -> Tracked {
        use BinaryOperator::*;
        let key = match (a.key(), b.key()) {
            (Some(Key::Int(a)), Some(Key::Int(b))) => match operator {
                Equal => Some(Key::Bool(a == b)),
                NotEqual => Some(Key::Bool(a != b)),
                Less => Some(Key::Bool(a < b)),
                LessEqual => Some(Key::Bool(a <= b)),
                Greater => Some(Key::Bool(a > b)),
                GreaterEqual => Some(Key::Bool(a >= b)),
                Add => a.checked_add(b).map(Key::Int),
                Subtract => a.checked_sub(b).map(Key::Int),
                Multiply => a.checked_mul(b).map(Key::Int),
                Divide => a.checked_div(b).map(Key::Int),
                Remainder => a.checked_rem(b).map(Key::Int),
                _ => None,
            },
            (Some(Key::Bool(a)), Some(Key::Bool(b))) => match operator {
                Equal => Some(Key::Bool(a == b)),
                NotEqual => Some(Key::Bool(a != b)),
                And => Some(Key::Bool(a && b)),
                Or => Some(Key::Bool(a || b)),
                _ => None,
            },
            _ => None,
        };
        Tracked::Data {
            effect: if let Some(Key::Bool(value)) = key {
                Value::Bool(Some(value))
            } else {
                self.typed(&self.type_at(expression.range))
            },
            key,
        }
    }
    fn map_expression(
        &self,
        expression: &Expression,
        state: State,
        map: impl Fn(Tracked) -> Tracked,
    ) -> Vec<Evaluation> {
        self.expression(expression, state)
            .into_iter()
            .map(|mut evaluated| {
                if let Ok(value) = evaluated.result {
                    evaluated.result = Ok(map(value));
                }
                evaluated
            })
            .collect()
    }
    pub fn sequence<'a>(
        &self,
        values: impl Iterator<Item = &'a Expression>,
        state: State,
    ) -> (Vec<(State, Vec<Tracked>)>, Vec<Evaluation>) {
        let mut normal = vec![(state, Vec::new())];
        let mut errors = Vec::new();
        for expression in values {
            let mut next = Vec::new();
            for (state, values) in normal {
                for evaluated in self.expression(expression, state) {
                    match evaluated.result {
                        Ok(value) => {
                            let mut values = values.clone();
                            values.push(value);
                            next.push((evaluated.state, values));
                        }
                        Err(_) => errors.push(evaluated),
                    }
                }
            }
            normal = next;
        }
        (normal, errors)
    }
    pub fn errors(&self, state: State, errors: &Language, token: Option<Token>) -> Vec<Evaluation> {
        errors
            .iter()
            .filter_map(|word| {
                Some(Evaluation::failed(
                    state.condition(&word.guards)?,
                    word.clone(),
                    token.clone(),
                ))
            })
            .collect()
    }
}

// Keep expression results distinct while applying the same conservative state
// meet as statement paths. Otherwise nested short-circuit expressions can
// duplicate an identical state at every operator, before reaching a statement.
fn coalesce(engine: &Engine<'_>, values: Vec<Evaluation>) -> Vec<Evaluation> {
    if values.len() < 2 {
        return values;
    }
    let paths = values
        .into_iter()
        .map(|value| Path {
            state: value.state,
            control: match value.result {
                Ok(value) => Control::Return(value),
                Err(stop) => stop.control(),
            },
        })
        .collect();
    engine
        .unique_paths(paths)
        .into_iter()
        .map(|path| Evaluation {
            state: path.state,
            result: match path.control {
                Control::Return(value) => Ok(value),
                Control::Error(word, token) => Err(Stop::Error(word, token)),
                Control::Diverges => Err(Stop::Diverges),
                Control::Panic => Err(Stop::Panic),
                Control::Recurse(held) => Err(Stop::Recurse(held)),
                _ => unreachable!("expression outcome is a value, failure or divergence"),
            },
        })
        .collect()
}
