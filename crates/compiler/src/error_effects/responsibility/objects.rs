//! Heap identity is separate from the task identities stored in its fields.
//! Summary snapshots are analysis data, not copies of runtime class instances.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct Object {
    pub ty: String,
    pub fields: BTreeMap<String, Tracked>,
}
impl State {
    pub fn join_objects(&mut self, other: &Self) {
        self.mutable_inputs |= other.mutable_inputs;
        self.unknown_async_mutation |= other.unknown_async_mutation;
        self.async_mutable.extend(other.async_mutable.clone());
        let left_population = self.task_population();
        let right_population = other.task_population();
        let left_pending = self.pending.keys().cloned().collect();
        let right_pending = other.pending.keys().cloned().collect();
        for (identity, object) in &other.objects {
            if self.objects.get(identity) == Some(object) {
                continue;
            }
            if let Some(previous) = self.objects.get_mut(identity) {
                for (name, value) in &object.fields {
                    if let Some(left) = previous.fields.get_mut(name) {
                        if left == value {
                            continue;
                        }
                        left.join_families(
                            value.clone(),
                            &left_pending,
                            &right_pending,
                            &left_population,
                            &right_population,
                        );
                    } else {
                        previous.fields.insert(name.clone(), value.clone());
                    }
                }
            } else {
                self.objects.insert(identity.clone(), object.clone());
            }
        }
    }
    pub fn object_tokens(&self) -> BTreeSet<Token> {
        let mut tokens: BTreeSet<_> = self
            .objects
            .values()
            .flat_map(|object| object.fields.values().flat_map(Tracked::tokens))
            .collect();
        tokens.extend(self.stores.values().flat_map(Tracked::tokens));
        tokens.extend(self.async_stores.values().flatten().cloned());
        tokens
    }
    pub fn freeze(&self, value: &Tracked) -> Tracked {
        fn visit(state: &State, value: &Tracked, seen: &mut BTreeSet<Token>) -> Tracked {
            let mut value = value.clone();
            if let Tracked::Reference {
                identity,
                ty,
                snapshot,
            } = &mut value
            {
                if !identity.observable() {
                    let held = state
                        .objects
                        .get(identity)
                        .map(|object| object.fields.values().flat_map(Tracked::tokens).collect())
                        .unwrap_or_default();
                    return Tracked::unknown(Value::Opaque(ty.clone()), held);
                }
                if seen.insert(identity.clone()) {
                    if let Some(object) = state.objects.get(identity) {
                        *snapshot = Some(
                            object
                                .fields
                                .iter()
                                .map(|(name, value)| (name.clone(), visit(state, value, seen)))
                                .collect(),
                        );
                    }
                    seen.remove(identity);
                }
                return value;
            }
            value.map_children(&mut |child| *child = visit(state, child, seen));
            value
        }
        visit(self, value, &mut BTreeSet::new())
    }
    pub fn fresh_object(&mut self, at: Site, slot: u32) -> (Token, bool) {
        let current = Token::Local(at, slot, false);
        let old = Token::Local(at, slot, true);
        let aged = self.objects.contains_key(&current);
        if let Some(mut object) = self.objects.remove(&current) {
            if let Some(previous) = self.objects.remove(&old) {
                for (name, value) in previous.fields {
                    if let Some(next) = object.fields.get_mut(&name) {
                        next.join(value);
                    }
                }
            }
            // A single reference to an older instance never represents all
            // the instances created at this allocation site.
            for value in object.fields.values_mut() {
                *value = Tracked::unknown(value.effect(), value.tokens());
            }
            for value in self.env.values_mut() {
                value.rename_object(&current, &old);
            }
            self.rename_watch_object(&current, &old);
            self.rename_mutation_object(&current, &old);
            for value in self.stores.values_mut() {
                value.rename_object(&current, &old);
            }
            for object in self.objects.values_mut() {
                for value in object.fields.values_mut() {
                    value.rename_object(&current, &old);
                }
            }
            for value in object.fields.values_mut() {
                value.rename_object(&current, &old);
            }
            if self.async_mutable.remove(&current) {
                self.async_mutable.insert(old.clone());
            }
            self.objects.insert(old, object);
        }
        (current, aged)
    }
}
impl Engine<'_> {
    pub fn stable_projection(&self, parameter: &Parameter) -> bool {
        parameter.path.iter().all(|access| {
            let Access::Field(owner, name) = access else {
                return true;
            };
            self.immutable_type(owner)
                || self.checked.source_types.get(owner).is_some_and(|schema| {
                    schema.kind == "error"
                        || schema
                            .fields
                            .iter()
                            .position(|(field, _)| field == name)
                            .is_some_and(|index| schema.const_fields.contains(&(index as u32)))
                })
        })
    }
    pub fn havoc_objects(&self, state: &mut State, asynchronous: bool) {
        state.invalidate_watches(None);
        state.taint_mutations(None);
        state.mutable_inputs = true;
        state.unknown_async_mutation |= asynchronous;
        for (field, value) in &mut state.stores {
            let ty = field
                .path
                .last()
                .and_then(|access| {
                    if let Access::Field(owner, name) = access {
                        self.checked
                            .source_types
                            .get(owner)
                            .and_then(|schema| {
                                schema.fields.iter().find(|(field, _)| field == name)
                            })
                            .map(|(_, ty)| ty)
                    } else {
                        None
                    }
                })
                .cloned()
                .unwrap_or(Type::Never);
            *value = Tracked::unknown(self.typed(&ty), value.tokens());
        }
        for (identity, object) in state.objects.iter_mut() {
            if asynchronous {
                state.async_mutable.insert(identity.clone());
                state.sticky_mutable.insert(identity.clone());
            }
            for (name, value) in &mut object.fields {
                let ty = self
                    .checked
                    .source_types
                    .get(&object.ty)
                    .and_then(|schema| schema.fields.iter().find(|(field, _)| field == name))
                    .map(|(_, ty)| ty.clone())
                    .unwrap_or(Type::Never);
                *value = Tracked::unknown(self.typed(&ty), value.tokens());
            }
        }
    }
    pub fn read_field(
        &self,
        state: &State,
        value: &Tracked,
        owner: &str,
        name: &str,
        ty: &Type,
    ) -> Tracked {
        if let Tracked::Reference { identity, .. } = value {
            if let Some(field) = state
                .objects
                .get(identity)
                .and_then(|object| object.fields.get(name))
            {
                return if identity.observable() {
                    field.clone()
                } else {
                    Tracked::unknown(self.typed(ty), field.tokens())
                };
            }
            return Tracked::unknown(self.typed(ty), BTreeSet::new());
        }
        let immutable = self.checked.source_types.get(owner).is_some_and(|schema| {
            schema.kind == "record"
                || schema.kind == "error"
                || schema
                    .fields
                    .iter()
                    .position(|(field, _)| field == name)
                    .is_some_and(|index| schema.const_fields.contains(&(index as u32)))
        }) || self.private_environment(owner);
        if let Tracked::Input { parameter, .. } = value {
            let mut field = parameter.clone();
            field
                .path
                .push(Access::Field(owner.to_owned(), name.to_owned()));
            if let Some(value) = state.stores.get(&field) {
                return value.clone();
            }
        }
        if immutable || matches!(value, Tracked::Input { .. }) && !state.mutable_inputs {
            value.project(
                Access::Field(owner.to_owned(), name.to_owned()),
                self.typed(ty),
                matches!(ty, Type::Future(_)),
            )
        } else {
            Tracked::unknown(self.typed(ty), state.freeze(value).tokens())
        }
    }
    pub fn write_field(&self, state: &mut State, base: &Tracked, name: &str, value: Tracked) {
        if let Tracked::Reference { identity, .. } = base {
            state.taint_mutations(Some(identity));
            state.invalidate_watches(Some(identity));
            if state.async_mutable.contains(identity) {
                let owner = base.effect().nominal().unwrap_or_default().to_owned();
                if self
                    .checked
                    .source_types
                    .get(&owner)
                    .and_then(|schema| schema.fields.iter().find(|(field, _)| field == name))
                    .is_some_and(|(_, ty)| self.contains_mutable_class(ty, &mut BTreeSet::new()))
                {
                    // A pending writer can obtain the newly shared object
                    // later through this field, including closure captures.
                    self.havoc_objects(state, true);
                }
            }
        }
        if let Tracked::Input {
            parameter, effect, ..
        } = base
        {
            state.invalidate_watches(None);
            // Two symbolic input objects of the same class can be aliases.
            // Their individual identities are resolved only at the caller.
            let affected: Vec<_> = state
                .objects
                .iter()
                .filter(|(identity, object)| {
                    matches!(identity, Token::Input(_))
                        && object.ty == effect.nominal().unwrap_or_default()
                })
                .map(|(identity, _)| identity.clone())
                .collect();
            for identity in affected {
                state.taint_mutations(Some(&identity));
            }
            let owner = effect.nominal().unwrap_or_default().to_owned();
            let mut field = parameter.clone();
            field
                .path
                .push(Access::Field(owner.clone(), name.to_owned()));
            for (other, stored) in &mut state.stores {
                if other != &field
                    && matches!(other.path.last(),Some(Access::Field(ty,field)) if ty==&owner&&field==name)
                {
                    let ty = self
                        .checked
                        .source_types
                        .get(&owner)
                        .and_then(|schema| schema.fields.iter().find(|(field, _)| field == name))
                        .map(|(_, ty)| ty.clone())
                        .unwrap_or(Type::Never);
                    *stored = Tracked::unknown(self.typed(&ty), stored.tokens());
                }
            }
            state.stores.insert(field, value);
            state.mutable_inputs = true;
            self.mutates.set(true);
            return;
        }
        if let Tracked::Reference { identity, .. } = base
            && identity.observable()
            && let Some(object) = state.objects.get_mut(identity)
        {
            let value = if state.async_mutable.contains(identity) {
                let ty = self
                    .checked
                    .source_types
                    .get(&object.ty)
                    .and_then(|schema| schema.fields.iter().find(|(field, _)| field == name))
                    .map(|(_, ty)| ty.clone())
                    .unwrap_or(Type::Never);
                Tracked::unknown(self.typed(&ty), value.tokens())
            } else {
                value
            };
            object.fields.insert(name.to_owned(), value);
        } else {
            self.mutates.set(true);
            self.havoc_objects(state, false);
        }
    }
}
impl Tracked {
    pub fn map_children(&mut self, visit: &mut impl FnMut(&mut Tracked)) {
        match self {
            Self::Reference {
                snapshot: Some(fields),
                ..
            }
            | Self::Record { fields, .. } => {
                for value in fields.values_mut() {
                    visit(value);
                }
            }
            Self::List(values)
            | Self::Batch(values)
            | Self::Choice(values)
            | Self::Variant { fields: values, .. } => {
                for value in values {
                    visit(value);
                }
            }
            Self::Map(entries) => {
                for (_, value) in entries {
                    visit(value);
                }
            }
            Self::Future { result, .. } => result.map_values(visit),
            Self::Iteration { base, .. }
            | Self::Keys(base)
            | Self::Length {
                collection: base, ..
            } => visit(base),
            _ => {}
        }
    }
    pub fn visit_objects(&mut self, visit: &mut impl FnMut(&mut Token)) {
        if let Self::Future { result, .. } = self {
            fn targets(result: &mut FutureResult, visit: &mut impl FnMut(&mut Token)) {
                if let FutureResult::Cases(cases) = result {
                    for case in cases {
                        if let Some(completion) = &mut case.completion {
                            completion.visit_objects(visit);
                        }
                        targets(&mut case.result, visit);
                    }
                }
            }
            targets(result, visit);
        }
        if let Self::Reference { identity, .. } = self {
            visit(identity);
        }
        self.map_children(&mut |child| child.visit_objects(visit));
    }
    pub fn rename_object(&mut self, current: &Token, old: &Token) {
        self.visit_objects(&mut |identity| {
            if identity == current {
                *identity = old.clone();
            }
        });
    }
}
