//! All-elements observations require a proof of exhaustive, completed traversal.
//! An ordinary unknown index still denotes just one possible element.
use super::*;
impl Engine<'_> {
    /// A map starting empty with a single insertion at each increasing Int
    /// counter cannot overwrite an earlier key. Other writes invalidate this
    /// proof, including nested writes and shadowing. Unknown keys stay opaque.
    pub(super) fn growing_map_inserts(
        &self,
        condition: &Expression,
        body: &Block,
        state: &State,
    ) -> Vec<(String, Site)> {
        let ExpressionKind::Binary {
            operator: BinaryOperator::Less,
            left,
            ..
        } = &condition.kind
        else {
            return Vec::new();
        };
        let ExpressionKind::Name(counter) = &left.kind else {
            return Vec::new();
        };
        if counter.len() != 1
            || self.type_at(left.range) != Type::Int
            || state.env.get(&counter[0]).and_then(Tracked::key) != Some(Key::Int(0))
            || !self.bounded_counter(condition, body)
        {
            return Vec::new();
        }
        let Some(Statement {
            kind: StatementKind::Assign { value, .. },
            ..
        }) = body.statements.last()
        else {
            return Vec::new();
        };
        if !matches!(&value.kind,ExpressionKind::Binary{operator:BinaryOperator::Add,right,..} if matches!(right.kind,ExpressionKind::Literal(Literal::Int(1))))
        {
            return Vec::new();
        }
        let mut result = Vec::new();
        for statement in &body.statements {
            let StatementKind::Assign { name, value } = &statement.kind else {
                continue;
            };
            if !matches!(state.env.get(name),Some(Tracked::Map(entries)) if entries.is_empty())
                || self.checked.data_operations.get(&value.range)
                    != Some(&crate::DataOperation::MapInsert)
            {
                continue;
            }
            let ExpressionKind::Call {
                arguments: Arguments::Positional(arguments),
                ..
            } = &value.kind
            else {
                continue;
            };
            if arguments.len() != 3
                || !matches!(&arguments[0].kind,ExpressionKind::Name(path) if path==&vec![name.clone()])
                || !matches!(&arguments[1].kind,ExpressionKind::Name(path) if path==counter)
                || !matches!(self.type_at(arguments[2].range), Type::Future(_))
            {
                continue;
            }
            struct Writes<'a> {
                name: &'a str,
                permitted: SourceRange,
                invalid: bool,
            }
            impl Visitor for Writes<'_> {
                fn enter_statement(&mut self, statement: &Statement) {
                    match &statement.kind {
                        StatementKind::Assign { name, .. }
                            if name == self.name && statement.range != self.permitted =>
                        {
                            self.invalid = true
                        }
                        StatementKind::Local { name, .. } if name == self.name => {
                            self.invalid = true
                        }
                        _ => {}
                    }
                }
            }
            let mut writes = Writes {
                name,
                permitted: statement.range,
                invalid: false,
            };
            ast_walk::block(&mut body.clone(), &mut writes);
            if !writes.invalid {
                result.push((name.clone(), self.at(value.range)));
            }
        }
        result
    }

    pub(super) fn exhaustive_list_loop(
        &self,
        condition: &Expression,
        body: &Block,
        state: &State,
        all: bool,
    ) -> Option<Vec<Path>> {
        let ExpressionKind::Binary {
            operator: BinaryOperator::Less,
            left,
            right,
        } = &condition.kind
        else {
            return None;
        };
        let ExpressionKind::Name(counter) = &left.kind else {
            return None;
        };
        if counter.len() != 1
            || state.env.get(&counter[0]).and_then(Tracked::key) != Some(Key::Int(0))
            || !self.bounded_counter(condition, body)
        {
            return None;
        }
        if let ExpressionKind::Name(bound) = &right.kind
            && bound.len() == 1
            && let Some(Tracked::Length {
                at,
                name,
                collection,
            }) = state.env.get(&bound[0])
        {
            // A copied length is tied to this immutable List value, never to
            // an arbitrary opaque value which merely has the same type.
            if !matches!(
                collection.as_ref(),
                Tracked::List(_)
                    | Tracked::Members(_)
                    | Tracked::Keys(_)
                    | Tracked::Input { .. }
                    | Tracked::Iteration { .. }
            ) || state.env.get(name) != Some(collection.as_ref())
            {
                return None;
            }
            let mut condition = condition.clone();
            if let ExpressionKind::Binary { right, .. } = &mut condition.kind {
                **right = Expression {
                    range: range(*at),
                    kind: ExpressionKind::Call {
                        target: vec!["listLength".into()],
                        arguments: Arguments::Positional(vec![Expression {
                            range: range(*at),
                            kind: ExpressionKind::Name(vec![name.clone()]),
                        }]),
                    },
                };
            }
            return self.exhaustive_list_loop(&condition, body, state, all);
        }
        let ExpressionKind::Call {
            arguments: Arguments::Positional(arguments),
            ..
        } = &right.kind
        else {
            return None;
        };
        if self.checked.data_operations.get(&right.range) != Some(&crate::DataOperation::ListLength)
        {
            return None;
        }
        let ExpressionKind::Name(collection) = &arguments.first()?.kind else {
            return None;
        };
        if collection.len() != 1 {
            return None;
        }
        let original = state.env.get(&collection[0])?.clone();
        if original == Tracked::Unbound {
            return None;
        }
        let last = body.statements.last()?;
        let StatementKind::Assign { value, .. } = &last.kind else {
            return None;
        };
        if !matches!(&value.kind,ExpressionKind::Binary{operator:BinaryOperator::Add,right,..} if matches!(right.kind,ExpressionKind::Literal(Literal::Int(1))))
        {
            return None;
        }
        let at = self.at(condition.range);
        let mut probe = state.clone();
        probe.iterated.retain(|(owner, _)| *owner != at);
        probe.env.insert(
            counter[0].clone(),
            Tracked::Data {
                effect: self.typed(&self.type_at(left.range)),
                key: Some(Key::Iteration(at)),
            },
        );
        probe.env.insert(
            collection[0].clone(),
            Tracked::Iteration {
                at,
                base: Box::new(original.clone()),
                path: Vec::new(),
                effect: original.effect(),
            },
        );
        // Buffers are not visible to source code. Start a fresh iteration delta
        // so an Error already collected before the loop is never repeated.
        for group in &mut probe.groups {
            group.errors = language::empty_word();
        }
        let paths = self.block(
            body,
            vec![Path {
                state: probe,
                control: Control::Normal,
            }],
            all,
        );
        let (mut blocked, paths): (Vec<_>, Vec<_>) = paths.into_iter().partition(|path| {
            path.control == Control::Diverges
                && path
                    .state
                    .guards
                    .values()
                    .any(|guard| matches!(guard, language::Guard::FutureNeverReturns))
        });
        let (mut early, paths): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .partition(|path| matches!(path.control, Control::Error(..) | Control::Panic));
        if paths.is_empty()
            || paths.iter().any(|path| {
                path.control != Control::Normal
                    || path.state.requirements != state.requirements
                    || path
                        .state
                        .pending
                        .keys()
                        .any(|token| !state.pending.contains_key(token))
            })
        {
            return None;
        }
        let mut observed: Option<BTreeSet<Vec<Access>>> = None;
        let mut grouped: Vec<Option<BTreeSet<Vec<Access>>>> = vec![None; state.groups.len()];
        let mut deltas = vec![Language::new(); state.groups.len()];
        let mut exit = state.clone();
        for path in paths {
            let current: BTreeSet<_> = path
                .state
                .iterated
                .iter()
                .filter(|(owner, _)| *owner == at)
                .map(|(_, path)| path.clone())
                .collect();
            intersect(&mut observed, current);
            for (index, group) in path.state.groups.iter().enumerate() {
                let seen = group
                    .observed
                    .iter()
                    .filter_map(|token| {
                        if let Token::Iteration(owner, path) = token {
                            (*owner == at).then(|| path.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                intersect(&mut grouped[index], seen);
                deltas[index].extend(language::independent_elements(&group.errors));
            }
            exit.join(&path.state);
        }
        let observed = observed?;
        if observed.is_empty() {
            return None;
        }
        for path in observed {
            let delegated = exit.receivers.values().any(|receiver| {
                receiver
                    .producers
                    .contains_key(&Token::Iteration(at, path.clone()))
            });
            for token in self.observation_tokens(&all_projection(&original, &path)) {
                if delegated {
                    exit.provisional_observe(&token);
                } else {
                    exit.complete_receiver(&token);
                    exit.observe(&token);
                }
            }
        }
        // One representative iteration registered one representative child.
        // Its receipt describes the entire traversal, while a single alias of
        // that child must never prove that every child has completed.
        for receiver in exit.receivers.values_mut() {
            let project = |token: &Token| {
                if let Token::Iteration(owner, path) = receivers::canonical(token)
                    && owner == at
                {
                    self.completion_tokens(&all_projection(&original, &path))
                } else {
                    BTreeSet::from([token.clone()])
                }
            };
            receiver.producers = std::mem::take(&mut receiver.producers)
                .into_iter()
                .flat_map(|(token, receipt)| {
                    project(&token)
                        .into_iter()
                        .map(move |token| (receivers::canonical(&token), receipt.clone()))
                })
                .collect();
            receiver.completes = receiver.completes.iter().flat_map(project).collect();
            receiver.covers = receiver
                .covers
                .iter()
                .flat_map(project)
                .map(|token| receivers::canonical(&token))
                .collect();
        }
        let stable = state
            .task_population()
            .into_iter()
            .chain(state.env.values().flat_map(Tracked::tokens))
            .chain(state.object_tokens())
            .collect();
        exit.age(&stable);
        for (index, delta) in deltas.into_iter().enumerate() {
            let mut closure = language::empty_word();
            loop {
                let mut next = closure.clone();
                next.extend(language::concatenate(&closure, &delta, self.bound));
                if next == closure {
                    break;
                }
                closure = next;
            }
            exit.groups[index].errors =
                language::concatenate(&state.groups[index].errors, &closure, self.bound);
            exit.groups[index]
                .observed
                .retain(|token| !matches!(token,Token::Iteration(owner,_) if *owner==at));
            for path in grouped[index].take().unwrap_or_default() {
                exit.groups[index]
                    .observed
                    .extend(self.observation_tokens(&all_projection(&original, &path)));
            }
        }
        exit.guards = state.guards.clone();
        exit.iterated.retain(|(owner, _)| *owner != at);
        for value in exit.env.values_mut() {
            value.finish_iteration(at);
        }
        exit.env.insert(collection[0].clone(), original.clone());
        for path in &mut blocked {
            // This representative element stops traversal. It cannot transfer
            // all of the collection's responsibilities to a blocked receiver.
            path.state.iterated.retain(|(owner, _)| *owner != at);
            for value in path.state.env.values_mut() {
                value.finish_iteration(at);
            }
            path.state
                .env
                .insert(collection[0].clone(), original.clone());
        }
        for path in &mut early {
            path.state.iterated.retain(|(owner, _)| *owner != at);
            for value in path.state.env.values_mut() {
                value.finish_iteration(at);
            }
            path.state
                .env
                .insert(collection[0].clone(), original.clone());
        }
        blocked.extend(early);
        blocked.push(Path {
            state: exit,
            control: Control::Normal,
        });
        Some(blocked)
    }
}
fn intersect<T: Ord>(common: &mut Option<BTreeSet<T>>, next: BTreeSet<T>) {
    if let Some(common) = common {
        common.retain(|item| next.contains(item));
    } else {
        *common = Some(next);
    }
}
fn all_projection(value: &Tracked, path: &[Access]) -> Tracked {
    let mut value = value.clone();
    for access in path {
        value = value.project(access.clone(), Value::Plain, true);
    }
    value
}
