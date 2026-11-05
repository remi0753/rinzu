//! All-elements observations require a proof of exhaustive, completed traversal.
//! An ordinary unknown index still denotes just one possible element.
use super::*;
impl Engine<'_> {
    pub(super) fn list_for(
        &self,
        binding: &str,
        iterable: &Expression,
        body: &Block,
        state: State,
        all: bool,
    ) -> Vec<Path> {
        let mut result = Vec::new();
        let at = self.at(iterable.range);
        let ty = self.type_at(iterable.range);
        let Type::List(element) = &ty else {
            return result;
        };
        for evaluated in self.expression(iterable, state) {
            let original = match evaluated.result {
                Ok(value) => value,
                Err(stop) => {
                    result.push(Path {
                        state: evaluated.state,
                        control: stop.control(),
                    });
                    continue;
                }
            };
            let state = evaluated.state;
            if let Tracked::List(elements) = &original
                && elements.len() <= 16
            {
                // Keep concrete aliases/closure variants for a small known
                // snapshot. The bound prevents compile-time path explosion;
                // larger or symbolic populations use the invariant proof below.
                let stable = state
                    .task_population()
                    .into_iter()
                    .chain(state.env.values().flat_map(Tracked::tokens))
                    .chain(state.object_tokens())
                    .collect();
                let mut active = vec![Path {
                    state,
                    control: Control::Normal,
                }];
                let mut exits = Vec::new();
                for element in elements {
                    let mut next = Vec::new();
                    for mut path in active {
                        if binding != "_" {
                            path.state.env.insert(binding.to_owned(), element.clone());
                        }
                        for mut path in self.block(body, vec![path], all) {
                            path.state.env.remove(binding);
                            match path.control {
                                Control::Normal | Control::Continue => {
                                    path.control = Control::Normal;
                                    path.state.age(&stable);
                                    next.push(path);
                                }
                                Control::Break => {
                                    path.control = Control::Normal;
                                    exits.push(path);
                                }
                                _ => exits.push(path),
                            }
                        }
                    }
                    active = self.unique_paths(next);
                }
                exits.extend(active);
                result.extend(exits);
                continue;
            }
            let mut probe = state.clone();
            probe.iterated.retain(|(owner, _)| *owner != at);
            probe
                .iteration_variants
                .retain(|(owner, _), _| *owner != at);
            let source = Tracked::Iteration {
                at,
                base: Box::new(original.clone()),
                path: Vec::new(),
                effect: original.effect(),
            };
            if binding != "_" {
                probe.env.insert(
                    binding.to_owned(),
                    source.project(
                        Access::Each(ty.to_string()),
                        self.typed(element),
                        matches!(element.as_ref(), Type::Future(_)),
                    ),
                );
            }
            for group in &mut probe.groups {
                group.errors = language::empty_word();
            }
            let mut paths = self.block(
                body,
                vec![Path {
                    state: probe,
                    control: Control::Normal,
                }],
                all,
            );
            for path in &mut paths {
                path.state.env.remove(binding);
                if path.control == Control::Continue {
                    path.control = Control::Normal;
                }
            }
            if let Some(paths) = self.complete_list_traversal(at, &original, &state, paths, None) {
                result.extend(paths);
                continue;
            }
            // A partial traversal proves no all-elements observation. Use the
            // same bounded state meet as an ordinary loop, retaining obligations.
            let mut header = state;
            let stable = header
                .task_population()
                .into_iter()
                .chain(header.env.values().flat_map(Tracked::tokens))
                .chain(header.object_tokens())
                .collect();
            loop {
                let before = header.clone();
                result.push(Path {
                    state: header.clone(),
                    control: Control::Normal,
                });
                let mut iteration = header.clone();
                if binding != "_" {
                    iteration.env.insert(
                        binding.to_owned(),
                        original.project(
                            Access::Element(ty.to_string(), None, at),
                            self.typed(element),
                            matches!(element.as_ref(), Type::Future(_)),
                        ),
                    );
                }
                for mut path in self.block(
                    body,
                    vec![Path {
                        state: iteration,
                        control: Control::Normal,
                    }],
                    all,
                ) {
                    path.state.env.remove(binding);
                    match path.control {
                        Control::Normal | Control::Continue => {
                            path.state.age(&stable);
                            header.join(&path.state);
                        }
                        Control::Break => {
                            path.control = Control::Normal;
                            result.push(path);
                        }
                        _ => result.push(path),
                    }
                }
                if header == before {
                    break;
                }
            }
        }
        self.unique_paths(result)
    }

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
            || !crate::collections::bounded_counter(self.checked, condition, body)
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
            || !crate::collections::bounded_counter(self.checked, condition, body)
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
                        target: vec![crate::DataOperation::ListLength.name().into()],
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
        self.complete_list_traversal(at, &original, state, paths, Some(&collection[0]))
    }

    fn complete_list_traversal(
        &self,
        at: Site,
        original: &Tracked,
        state: &State,
        paths: Vec<Path>,
        collection_name: Option<&str>,
    ) -> Option<Vec<Path>> {
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
        let mut observations = Vec::new();
        for path in paths {
            let current: BTreeSet<_> = path
                .state
                .iterated
                .iter()
                .filter(|(owner, _)| *owner == at)
                .map(|(_, path)| path.clone())
                .collect();
            observations.push((current.clone(), path.state.iteration_variants.clone()));
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
        let mut observed = observed?;
        let candidates: BTreeSet<_> = observations
            .iter()
            .flat_map(|(paths, _)| paths.iter().cloned())
            .collect();
        observed.extend(candidates.into_iter().filter(|candidate| {
            observations.iter().all(|(seen, variants)| {
                seen.contains(candidate) || projection_absent(at, candidate, variants)
            })
        }));
        if observed.is_empty() {
            return None;
        }
        for path in observed {
            let delegated = exit.receivers.values().any(|receiver| {
                receiver
                    .producers
                    .contains_key(&Token::Iteration(at, path.clone()))
            });
            for token in self.observation_tokens(&all_projection(original, &path)) {
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
                    self.completion_tokens(&all_projection(original, &path))
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
                next.extend(concatenate(&closure, &delta, self.bound));
                if next == closure {
                    break;
                }
                closure = next;
            }
            exit.groups[index].errors =
                concatenate(&state.groups[index].errors, &closure, self.bound);
            exit.groups[index]
                .observed
                .retain(|token| !matches!(token,Token::Iteration(owner,_) if *owner==at));
            for path in grouped[index].take().unwrap_or_default() {
                exit.groups[index]
                    .observed
                    .extend(self.observation_tokens(&all_projection(original, &path)));
            }
        }
        exit.guards = state.guards.clone();
        exit.iterated.retain(|(owner, _)| *owner != at);
        exit.iteration_variants.retain(|(owner, _), _| *owner != at);
        for value in exit.env.values_mut() {
            value.finish_iteration(at);
        }
        if let Some(name) = collection_name {
            exit.env.insert(name.to_owned(), original.clone());
        }
        for path in &mut blocked {
            // This representative element stops traversal. It cannot transfer
            // all of the collection's responsibilities to a blocked receiver.
            path.state.iterated.retain(|(owner, _)| *owner != at);
            path.state
                .iteration_variants
                .retain(|(owner, _), _| *owner != at);
            for value in path.state.env.values_mut() {
                value.finish_iteration(at);
            }
            if let Some(name) = collection_name {
                path.state.env.insert(name.to_owned(), original.clone());
            }
        }
        for path in &mut early {
            path.state.iterated.retain(|(owner, _)| *owner != at);
            path.state
                .iteration_variants
                .retain(|(owner, _), _| *owner != at);
            for value in path.state.env.values_mut() {
                value.finish_iteration(at);
            }
            if let Some(name) = collection_name {
                path.state.env.insert(name.to_owned(), original.clone());
            }
        }
        blocked.extend(early);
        blocked.push(Path {
            state: exit,
            control: Control::Normal,
        });
        Some(blocked)
    }
}
fn projection_absent(
    at: Site,
    path: &[Access],
    variants: &BTreeMap<(Site, Vec<Access>), BTreeSet<String>>,
) -> bool {
    for (index, access) in path.iter().enumerate() {
        if let Access::Variant(_, variant, _) = access
            && variants
                .get(&(at, path[..index].to_vec()))
                .is_some_and(|known| !known.contains(variant))
        {
            return true;
        }
    }
    false
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
