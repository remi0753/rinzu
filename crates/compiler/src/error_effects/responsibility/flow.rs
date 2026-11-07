use super::*;
impl Engine<'_> {
    pub(super) fn unique_paths(&self, paths: Vec<Path>) -> Vec<Path> {
        // Keep the actual aliases, but bound combinations of independent
        // branches with a conservative meet at a control
        // boundary instead of carrying their Cartesian product into callers.
        // This is the same state meet used for loop backedges: obligations
        // union, observations intersect, and changed field facts become unknown.
        if paths.len() > 32 {
            let mut joined: Vec<Path> = Vec::new();
            for path in paths {
                if let Some(previous) = joined.iter_mut().find(|previous| {
                    previous.control == path.control
                        || (boolean_result(&previous.control).is_some()
                            && boolean_result(&path.control).is_some())
                }) {
                    join_path(previous, &path);
                } else {
                    joined.push(path);
                }
            }
            unique(joined)
        } else {
            unique(paths)
        }
    }
    pub fn block(&self, block: &Block, mut paths: Vec<Path>, all: bool) -> Vec<Path> {
        let outer: BTreeSet<_> = paths
            .iter()
            .flat_map(|path| path.state.env.keys().cloned())
            .collect();
        for statement in &block.statements {
            let mut next = Vec::new();
            for path in paths {
                if path.control == Control::Normal {
                    next.extend(self.statement(statement, path.state, all));
                } else {
                    next.push(path);
                }
            }
            if all {
                for path in &mut next {
                    if let Control::Error(word, token) = &path.control {
                        let group = path.state.groups.last_mut().expect("lexical Error group");
                        if token.as_ref().is_none_or(|token| {
                            !token.observable() || group.observed.insert(token.clone())
                        }) {
                            group.errors = concatenate(
                                &group.errors,
                                &Language::from([word.clone()]),
                                self.bound,
                            );
                        }
                        path.control = Control::Normal;
                    }
                }
            }
            // Terminal paths cannot read this block's locals again. Apply the
            // existing block-exit environment cleanup before deduplication.
            for path in &mut next {
                if path.control != Control::Normal {
                    path.state.env.retain(|name, _| outer.contains(name));
                }
            }
            paths = self.unique_paths(next);
        }
        for path in &mut paths {
            path.state.env.retain(|name, _| outer.contains(name));
        }
        self.unique_paths(paths)
    }
    fn statement(&self, statement: &Statement, state: State, all: bool) -> Vec<Path> {
        match &statement.kind {
            StatementKind::Block(block) => self.block(
                block,
                vec![Path {
                    state,
                    control: Control::Normal,
                }],
                all,
            ),
            StatementKind::Local {
                name, initializer, ..
            }
            | StatementKind::Assign {
                name,
                value: initializer,
            } => self
                .expression(initializer, state)
                .into_iter()
                .map(|evaluated| match evaluated.result {
                    Ok(value) => {
                        let mut state = evaluated.state;
                        state.env.insert(name.clone(), value.finite());
                        Path {
                            state,
                            control: Control::Normal,
                        }
                    }
                    Err(stop) => {
                        let mut state = evaluated.state;
                        if matches!(statement.kind, StatementKind::Local { .. }) {
                            state.env.insert(name.clone(), Tracked::Unbound);
                        }
                        Path {
                            state,
                            control: stop.control(),
                        }
                    }
                })
                .collect(),
            StatementKind::Expression(expression) => self
                .expression(expression, state)
                .into_iter()
                .map(expressions::Evaluation::path)
                .collect(),
            StatementKind::Throw(expression) => {
                let mut result = Vec::new();
                for evaluated in self.expression(expression, state) {
                    match evaluated.result {
                        Ok(value) => {
                            for word in value.effect().thrown_errors() {
                                if let Some(state) = evaluated.state.condition(&word.guards) {
                                    result.push(Path {
                                        state,
                                        control: Control::Error(word, None),
                                    });
                                }
                            }
                        }
                        Err(stop) => result.push(Path {
                            state: evaluated.state,
                            control: stop.control(),
                        }),
                    }
                }
                result
            }
            StatementKind::Return(expression) => expression.as_ref().map_or_else(
                || {
                    vec![Path {
                        state: state.clone(),
                        control: Control::Return(Tracked::data(Value::Plain)),
                    }]
                },
                |expression| {
                    self.expression(expression, state.clone())
                        .into_iter()
                        .map(|evaluated| Path {
                            state: evaluated.state,
                            control: match evaluated.result {
                                Ok(value) => Control::Return(value),
                                Err(stop) => stop.control(),
                            },
                        })
                        .collect()
                },
            ),
            StatementKind::Break => vec![Path {
                state,
                control: Control::Break,
            }],
            StatementKind::Continue => vec![Path {
                state,
                control: Control::Continue,
            }],
            StatementKind::FieldAssign { base, field, value } => {
                let (normal, mut result) = self.sequence([base, value].into_iter(), state);
                for (mut state, values) in normal {
                    self.write_field(&mut state, &values[0], field, values[1].clone());
                    result.push(expressions::Evaluation::pure(
                        state,
                        Tracked::data(Value::Plain),
                    ));
                }
                result
                    .into_iter()
                    .map(expressions::Evaluation::path)
                    .collect()
            }
            StatementKind::If {
                condition,
                then_block,
                else_branch,
            } => {
                let mut result = Vec::new();
                for evaluated in self.expression(condition, state) {
                    match evaluated.result {
                        Err(stop) => result.push(Path {
                            state: evaluated.state,
                            control: stop.control(),
                        }),
                        Ok(value) => {
                            for yes in [true, false] {
                                for guards in value.effect().bool_guards(yes) {
                                    if let Some(state) = evaluated.state.condition(&guards) {
                                        if yes {
                                            result.extend(self.block(
                                                then_block,
                                                vec![Path {
                                                    state,
                                                    control: Control::Normal,
                                                }],
                                                all,
                                            ));
                                        } else if let Some(branch) = else_branch {
                                            result.extend(self.statement(branch, state, all));
                                        } else {
                                            result.push(Path {
                                                state,
                                                control: Control::Normal,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                result
            }
            StatementKind::While { condition, body } => {
                self.while_loop(condition, body, state, all)
            }
            StatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => self.list_for(binding, iterable, body, state, all),
            StatementKind::Switch { value, cases } => {
                let mut result = Vec::new();
                let ty = self.type_at(value.range);
                for evaluated in self.expression(value, state) {
                    match evaluated.result {
                        Err(stop) => result.push(Path {
                            state: evaluated.state,
                            control: stop.control(),
                        }),
                        Ok(value) => {
                            for candidate in value.alternatives() {
                                for case in cases {
                                    let variant = case.path.last().unwrap();
                                    for guards in candidate.effect().selection_guards(variant) {
                                        if let Some(mut state) = evaluated.state.condition(&guards)
                                        {
                                            if let Tracked::Iteration { at, path, .. } = &candidate
                                            {
                                                let key = (*at, path.clone());
                                                if state
                                                    .iteration_variants
                                                    .get(&key)
                                                    .is_some_and(|known| !known.contains(variant))
                                                {
                                                    continue;
                                                }
                                                state
                                                    .iteration_variants
                                                    .insert(key, BTreeSet::from([variant.clone()]));
                                            }
                                            let outer: BTreeSet<_> =
                                                state.env.keys().cloned().collect();
                                            for (index, binding) in case.bindings.iter().enumerate()
                                            {
                                                let field = self
                                                    .checked
                                                    .source_types
                                                    .get(&ty.to_string())
                                                    .and_then(|schema| {
                                                        schema
                                                            .variants
                                                            .iter()
                                                            .find(|(name, _)| name == variant)
                                                    })
                                                    .and_then(|(_, fields)| fields.get(index))
                                                    .map(|(_, ty)| ty.clone())
                                                    .unwrap_or(Type::Never);
                                                state.env.insert(
                                                    binding.clone(),
                                                    candidate.project(
                                                        Access::Variant(
                                                            ty.to_string(),
                                                            variant.clone(),
                                                            index,
                                                        ),
                                                        self.typed(&field),
                                                        matches!(field, Type::Future(_)),
                                                    ),
                                                );
                                            }
                                            let mut paths = self.block(
                                                &case.body,
                                                vec![Path {
                                                    state,
                                                    control: Control::Normal,
                                                }],
                                                all,
                                            );
                                            for path in &mut paths {
                                                path.state
                                                    .env
                                                    .retain(|name, _| outer.contains(name));
                                            }
                                            result.extend(paths);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                result
            }
            StatementKind::TryErrors {
                all: collect,
                body,
                handlers,
                cleanup,
            } => self.try_block(body, handlers, cleanup.as_ref(), state, *collect),
            StatementKind::TryCatch {
                body,
                binding,
                binding_range,
                handler,
                cleanup,
            } => self.try_block(
                body,
                &[CatchClause::whole(binding, *binding_range, handler)],
                cleanup.as_ref(),
                state,
                false,
            ),
            StatementKind::TryAll {
                body,
                handler,
                cleanup,
            } => self.try_block(
                body,
                &handler
                    .iter()
                    .map(|handler| {
                        CatchClause::whole(&handler.binding, handler.binding_range, &handler.body)
                    })
                    .collect::<Vec<_>>(),
                cleanup.as_ref(),
                state,
                true,
            ),
            StatementKind::TryFinally { body, cleanup } => {
                self.try_block(body, &[], Some(cleanup), state, false)
            }
            StatementKind::TaskScope { body, cleanup } => {
                self.try_block(body, &[], Some(cleanup), state, true)
            }
        }
    }
    fn while_loop(
        &self,
        condition: &Expression,
        body: &Block,
        mut state: State,
        all: bool,
    ) -> Vec<Path> {
        let previous = state.map_appends.clone();
        for (name, at) in self.growing_map_inserts(condition, body, &state) {
            state.map_appends.insert(at);
            state
                .env
                .insert(name, Tracked::Members(FutureMembers::empty_map(at)));
        }
        let mut paths = self.while_paths(condition, body, state, all);
        for path in &mut paths {
            path.state.map_appends = previous.clone();
        }
        paths
    }
    fn while_paths(
        &self,
        condition: &Expression,
        body: &Block,
        state: State,
        all: bool,
    ) -> Vec<Path> {
        if let Some(paths) = self.exhaustive_list_loop(condition, body, &state, all) {
            return paths;
        }
        let mut header = state;
        let mut stable: BTreeSet<_> = header.pending.keys().cloned().collect();
        stable.extend(header.object_tokens());
        stable.extend(header.env.values().flat_map(Tracked::tokens));
        stable.extend(
            header
                .groups
                .iter()
                .flat_map(|group| group.observed.iter().cloned()),
        );
        let mut exits = Vec::new();
        let mut backedge = false;
        loop {
            let before = header.clone();
            for evaluated in self.expression(condition, header.clone()) {
                match evaluated.result {
                    Err(stop) => exits.push(Path {
                        state: evaluated.state,
                        control: stop.control(),
                    }),
                    Ok(value) => {
                        for yes in [true, false] {
                            for guards in value.effect().bool_guards(yes) {
                                if let Some(state) = evaluated.state.condition(&guards) {
                                    if !yes {
                                        exits.push(Path {
                                            state,
                                            control: Control::Normal,
                                        });
                                        continue;
                                    }
                                    for mut path in self.block(
                                        body,
                                        vec![Path {
                                            state,
                                            control: Control::Normal,
                                        }],
                                        all,
                                    ) {
                                        match path.control {
                                            Control::Normal | Control::Continue => {
                                                backedge = true;
                                                path.state.age(&stable);
                                                header.join(&path.state)
                                            }
                                            Control::Break => {
                                                path.control = Control::Normal;
                                                exits.push(path);
                                            }
                                            _ => exits.push(path),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Loop iterations revisit the same exits with a wider header.
            // Coalesce them before another iteration can multiply the states
            // sent through the surrounding catch and cleanup blocks.
            exits = self.unique_paths(exits);
            if header == before {
                break;
            }
        }
        if backedge && !crate::collections::bounded_counter(self.checked, condition, body) {
            exits.push(Path {
                state: header,
                control: Control::Diverges,
            });
        }
        self.unique_paths(exits)
    }
    fn try_block(
        &self,
        body: &Block,
        handlers: &[CatchClause],
        cleanup: Option<&Block>,
        state: State,
        all: bool,
    ) -> Vec<Path> {
        let paths = if all {
            self.collect_block(body, state)
        } else {
            self.block(
                body,
                vec![Path {
                    state,
                    control: Control::Normal,
                }],
                false,
            )
            .into_iter()
            .map(|path| (path, None))
            .collect()
        };
        let mut result = Vec::new();
        for (path, saved) in paths {
            let Control::Error(word, _) = &path.control else {
                result.push(path);
                continue;
            };
            let mut remaining = Language::from([word.clone()]);
            for handler in handlers {
                let catch_pattern = pattern(self.checked, &handler.pattern);
                for word in remaining.clone() {
                    let matched = catch_pattern.matches(&word);
                    if matched == Match::Never {
                        continue;
                    }
                    let mut state = path.state.clone();
                    let outer: BTreeSet<_> = state.env.keys().cloned().collect();
                    match &handler.pattern {
                        CatchPattern::Binding(binding) => {
                            state
                                .env
                                .insert(binding.name.clone(), Tracked::data(Value::Plain));
                        }
                        CatchPattern::List { elements, rest, .. } => {
                            for (index, element) in elements.iter().enumerate() {
                                if let Some(binding) = &element.binding {
                                    let tag = if catch_pattern.prefix[index] == Tag::Any {
                                        word.prefix.get(index).cloned().unwrap_or(Tag::Any)
                                    } else {
                                        catch_pattern.prefix[index].clone()
                                    };
                                    state.env.insert(
                                        binding.name.clone(),
                                        Tracked::data(Value::Error(BTreeSet::from([tag]))),
                                    );
                                }
                            }
                            if let Some(rest) = rest {
                                state
                                    .env
                                    .insert(rest.name.clone(), Tracked::data(Value::Plain));
                            }
                        }
                    }
                    for mut caught in self.block(
                        &handler.body,
                        vec![Path {
                            state,
                            control: Control::Normal,
                        }],
                        false,
                    ) {
                        caught.state.env.retain(|name, _| outer.contains(name));
                        if caught.control == Control::Normal
                            && let Some(saved) = &saved
                        {
                            caught.control = saved.clone();
                        }
                        result.push(caught);
                    }
                }
                remaining = language::subtract(&remaining, &catch_pattern);
                if remaining.is_empty() {
                    break;
                }
            }
            for word in remaining {
                let mut residual = path.clone();
                if let Control::Error(_, token) = &path.control {
                    residual.control = Control::Error(word, token.clone());
                }
                result.push(residual);
            }
        }
        let result = self.unique_paths(result);
        let Some(cleanup) = cleanup else {
            return result;
        };
        let mut cleaned = Vec::new();
        for incoming in result {
            if matches!(incoming.control, Control::Diverges | Control::Recurse(_)) {
                cleaned.push(incoming);
                continue;
            }
            for mut path in self.block(
                cleanup,
                vec![Path {
                    state: incoming.state,
                    control: Control::Normal,
                }],
                false,
            ) {
                match (&incoming.control, &path.control) {
                    (Control::Panic, Control::Diverges | Control::Recurse(_)) => {}
                    (Control::Panic, _) => path.control = Control::Panic,
                    (_, Control::Normal) => path.control = incoming.control.clone(),
                    (Control::Error(first, _), Control::Error(second, _)) => {
                        let errors = concatenate(
                            &Language::from([first.clone()]),
                            &Language::from([second.clone()]),
                            self.bound,
                        );
                        for word in errors {
                            let mut path = path.clone();
                            path.control = Control::Error(word, None);
                            cleaned.push(path);
                        }
                        continue;
                    }
                    _ => {}
                }
                cleaned.push(path);
            }
        }
        self.unique_paths(cleaned)
    }
    fn collect_block(&self, body: &Block, mut state: State) -> Vec<(Path, Option<Control>)> {
        state.groups.push(Group {
            errors: language::empty_word(),
            observed: BTreeSet::new(),
        });
        let mut result = Vec::new();
        for mut path in self.block(
            body,
            vec![Path {
                state,
                control: Control::Normal,
            }],
            true,
        ) {
            let group = path.state.groups.pop().expect("lexical Error group");
            if matches!(
                path.control,
                Control::Diverges | Control::Panic | Control::Recurse(_)
            ) {
                result.push((path, None));
                continue;
            }
            for word in group.errors {
                if word.prefix.is_empty() && !word.longer {
                    result.push((path.clone(), None));
                } else {
                    let saved = path.control.clone();
                    let mut failed = path.clone();
                    failed.control = Control::Error(word, None);
                    result.push((failed, Some(saved)));
                }
            }
        }
        result
    }
}
// The index is an accelerator only: collisions still use the original
// equivalence relation, and representatives retain their first-seen order.
fn same_path(previous: &Path, path: &Path) -> bool {
    let left = &previous.state;
    let right = &path.state;
    (previous.control == path.control
        || (boolean_result(&previous.control).is_some() && boolean_result(&path.control).is_some()))
        && left.env == right.env
        && left.pending == right.pending
        && left.observed == right.observed
        && left.iterated == right.iterated
        && left.requirements == right.requirements
        && left.guards == right.guards
        && left.objects == right.objects
        && left.stores == right.stores
        && left.async_stores == right.async_stores
        && left.async_mutable == right.async_mutable
        && left.mutable_inputs == right.mutable_inputs
        && left.watches == right.watches
        && left.field_watch_blocked == right.field_watch_blocked
        && left.mutations == right.mutations
        && left.sticky_mutable == right.sticky_mutable
        && left.groups.len() == right.groups.len()
        && left
            .groups
            .iter()
            .zip(&right.groups)
            .all(|(left, right)| left.observed == right.observed)
}
// A plain Boolean cannot carry task identities or heap aliases. With the same
// observation/mutation state, its two truth values can be joined exactly. This
// also prevents each operator of a long short-circuit expression from cloning
// all the live Future outcomes again. Conditional/symbolic values stay distinct.
fn boolean_result(control: &Control) -> Option<Option<bool>> {
    match control {
        Control::Return(Tracked::Data {
            effect: Value::Bool(value),
            ..
        }) => Some(*value),
        _ => None,
    }
}
fn join_path(previous: &mut Path, path: &Path) {
    if let (Some(left), Some(right)) = (
        boolean_result(&previous.control),
        boolean_result(&path.control),
    ) {
        previous.control = Control::Return(Tracked::data(Value::Bool(if left == right {
            left
        } else {
            None
        })));
    }
    previous.state.join(&path.state);
}
fn path_hash(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let state = &path.state;
    if boolean_result(&path.control).is_some() {
        Control::Return(Tracked::data(Value::Bool(None))).hash(&mut hash);
    } else {
        path.control.hash(&mut hash);
    }
    state.env.hash(&mut hash);
    state.pending.hash(&mut hash);
    state.observed.hash(&mut hash);
    state.iterated.hash(&mut hash);
    state.requirements.hash(&mut hash);
    state.guards.hash(&mut hash);
    state.objects.hash(&mut hash);
    state.stores.hash(&mut hash);
    state.async_stores.hash(&mut hash);
    state.async_mutable.hash(&mut hash);
    state.mutable_inputs.hash(&mut hash);
    state.watches.hash(&mut hash);
    state.field_watch_blocked.hash(&mut hash);
    state.mutations.hash(&mut hash);
    state.sticky_mutable.hash(&mut hash);
    state.groups.len().hash(&mut hash);
    for group in &state.groups {
        group.observed.hash(&mut hash);
    }
    hash.finish()
}
pub(super) fn unique(paths: Vec<Path>) -> Vec<Path> {
    // Tiny sets are cheaper to compare directly than to hash deep Future
    // outcomes. The threshold chooses an algorithm, never drops a path.
    if paths.len() < 32 {
        let mut unique: Vec<Path> = Vec::new();
        for path in paths {
            if let Some(previous) = unique
                .iter_mut()
                .find(|previous| same_path(previous, &path))
            {
                join_path(previous, &path);
            } else {
                unique.push(path);
            }
        }
        return unique;
    }
    unique_indexed(paths, path_hash)
}
fn unique_indexed(paths: Vec<Path>, hash_key: impl Fn(&Path) -> u64) -> Vec<Path> {
    if paths.len() < 2 {
        return paths;
    }
    let mut unique: Vec<Path> = Vec::new();
    let mut index: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for path in paths {
        let hash = hash_key(&path);
        let previous = index.get(&hash).and_then(|candidates| {
            candidates
                .iter()
                .copied()
                .find(|candidate| same_path(&unique[*candidate], &path))
        });
        if let Some(previous) = previous {
            join_path(&mut unique[previous], &path);
            // Join may change a representative's key as observation/mutation
            // facts meet. Keep it searchable under the current equivalence key.
            let next = hash_key(&unique[previous]);
            if next != hash {
                index
                    .get_mut(&hash)
                    .unwrap()
                    .retain(|candidate| *candidate != previous);
                let bucket = index.entry(next).or_default();
                let at = bucket.binary_search(&previous).unwrap_or_else(|at| at);
                bucket.insert(at, previous);
            }
        } else {
            index.entry(hash).or_default().push(unique.len());
            unique.push(path);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_paths_keep_pending_obligations_when_hashes_collide() {
        let word = |name: &str| language::Word {
            tail: BTreeSet::new(),
            prefix: vec![Tag::Named(name.into())],
            longer: false,
            guards: BTreeMap::new(),
        };
        let mut first = Path {
            state: State::default(),
            control: Control::Normal,
        };
        first.state.groups.push(Group {
            errors: Language::from([word("A")]),
            observed: BTreeSet::new(),
        });
        let mut pending = first.clone();
        pending.state.pending.insert(
            Token::Local((1, 2, 3, 0, 0), 0, false),
            Obligation {
                origin: (1, 2, 3, 0, 0),
                errors: Language::from([word("B")]),
            },
        );
        let mut following = first.clone();
        following.state.groups[0].errors = Language::from([word("B")]);
        let mut expected = first.clone();
        expected.state.join(&following.state);
        let paths = vec![first, pending.clone(), following];
        assert_eq!(
            unique_indexed(paths.clone(), |_| 0),
            vec![expected.clone(), pending.clone()]
        );
        assert_eq!(unique(paths), vec![expected, pending]);
    }
    #[test]
    fn indexed_paths_keep_the_first_seen_control_order() {
        let normal = Path {
            state: State::default(),
            control: Control::Normal,
        };
        let returned = Path {
            state: State::default(),
            control: Control::Return(Tracked::data(Value::Plain)),
        };
        assert_eq!(
            unique_indexed(
                vec![
                    returned.clone(),
                    normal.clone(),
                    returned.clone(),
                    normal.clone()
                ],
                |_| 0
            ),
            vec![returned, normal]
        );
    }
}
