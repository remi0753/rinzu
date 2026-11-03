//! Deferred field facts become current only on an unambiguous completion.
//! An intervening store or overlapping writer permanently taints that snapshot.
use super::*;
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct Completion {
    pub objects: BTreeMap<Token, objects::Object>,
    pub observed: BTreeSet<Token>,
    pub active: bool,
    pub tainted: BTreeSet<Token>,
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct Mutation {
    pub objects: BTreeSet<Token>,
    pub tainted: BTreeSet<Token>,
}
impl Completion {
    pub fn forget_nested_completions(&mut self) {
        fn strip(value: &mut Tracked) {
            if let Tracked::Future { result, .. } = value {
                fn clear(result: &mut FutureResult) {
                    if let FutureResult::Cases(cases) = result {
                        for case in cases {
                            case.completion = None;
                            clear(&mut case.result);
                        }
                    }
                }
                clear(result);
            }
            value.map_children(&mut strip);
        }
        self.map_values(&mut strip);
    }
    pub fn tokens(&self) -> BTreeSet<Token> {
        self.objects
            .values()
            .flat_map(|object| object.fields.values().flat_map(Tracked::tokens))
            .chain(self.observed.iter().cloned())
            .collect()
    }
    pub fn map_values(&mut self, visit: &mut impl FnMut(&mut Tracked)) {
        for object in self.objects.values_mut() {
            for value in object.fields.values_mut() {
                visit(value);
            }
        }
    }
    pub fn visit_tokens(&mut self, visit: &mut impl FnMut(&mut Token, &mut bool)) {
        self.map_values(&mut |value| value.visit_tokens(visit));
        self.observed = std::mem::take(&mut self.observed)
            .into_iter()
            .map(|mut token| {
                visit(&mut token, &mut false);
                token
            })
            .collect();
    }
    pub fn visit_objects(&mut self, visit: &mut impl FnMut(&mut Token)) {
        self.objects = std::mem::take(&mut self.objects)
            .into_iter()
            .map(|(mut token, object)| {
                visit(&mut token);
                (token, object)
            })
            .collect();
        self.tainted = std::mem::take(&mut self.tainted)
            .into_iter()
            .map(|mut token| {
                visit(&mut token);
                token
            })
            .collect();
    }
}
impl FutureResult {
    pub fn active_completion(&self) -> Option<Mutation> {
        let Self::Cases(cases) = self else {
            return None;
        };
        let mut mutation = Mutation {
            objects: BTreeSet::new(),
            tainted: BTreeSet::new(),
        };
        for case in cases {
            if let Some(completion) = &case.completion
                && completion.active
            {
                mutation.objects.extend(completion.objects.keys().cloned());
                mutation.tainted.extend(completion.tainted.clone());
            }
            if let Some(nested) = case.result.active_completion() {
                mutation.objects.extend(nested.objects);
                mutation.tainted.extend(nested.tainted);
            }
        }
        (!mutation.objects.is_empty()).then_some(mutation)
    }
}
impl State {
    pub fn exports_mutations(&self, returned: Option<&Tracked>) -> bool {
        if self.unknown_async_mutation || !self.sticky_mutable.is_empty() {
            return false;
        }
        let handles = returned.map(public_futures).unwrap_or_default();
        if !self.mutations.keys().all(|owner| handles.contains(owner)) {
            return false;
        }
        if self.mutations.is_empty() {
            return true;
        }
        // Every external write must already be covered by a returned active
        // child. Otherwise the parent still needs its own completion record.
        self.stores.keys().all(|field| {
            let mut base = field.clone();
            base.path.pop();
            self.mutations
                .values()
                .any(|mutation| mutation.objects.contains(&Token::Input(base.clone())))
        })
    }
    pub fn deferred_guarantees(&self, value: &Tracked) -> BTreeSet<Token> {
        fn visible(value: &Tracked, objects: &mut BTreeSet<Token>, futures: &mut BTreeSet<Token>) {
            match value {
                Tracked::Future { token, result, .. } => {
                    futures.insert(token.clone());
                    fn outcomes(
                        result: &FutureResult,
                        objects: &mut BTreeSet<Token>,
                        futures: &mut BTreeSet<Token>,
                    ) {
                        match result {
                            FutureResult::Success(value) => visible(value, objects, futures),
                            FutureResult::Cases(cases) => {
                                let mut common_objects: Option<BTreeSet<Token>> = None;
                                let mut common_futures: Option<BTreeSet<Token>> = None;
                                for case in cases {
                                    let mut branch_objects = BTreeSet::new();
                                    let mut branch_futures = BTreeSet::new();
                                    outcomes(
                                        &case.result,
                                        &mut branch_objects,
                                        &mut branch_futures,
                                    );
                                    intersect(&mut common_objects, branch_objects);
                                    intersect(&mut common_futures, branch_futures);
                                }
                                objects.extend(common_objects.unwrap_or_default());
                                futures.extend(common_futures.unwrap_or_default());
                            }
                            _ => {}
                        }
                    }
                    outcomes(result, objects, futures);
                }
                Tracked::Reference {
                    identity, snapshot, ..
                } => {
                    objects.insert(identity.clone());
                    if let Some(fields) = snapshot {
                        for value in fields.values() {
                            visible(value, objects, futures);
                        }
                    }
                }
                Tracked::Record { fields, .. } => {
                    for value in fields.values() {
                        visible(value, objects, futures);
                    }
                }
                Tracked::List(values) | Tracked::Variant { fields: values, .. } => {
                    for value in values {
                        visible(value, objects, futures);
                    }
                }
                Tracked::Map(entries) => {
                    for (_, value) in entries {
                        visible(value, objects, futures);
                    }
                }
                _ => {}
            }
        }
        fn snapshots(
            result: &FutureResult,
            objects: &BTreeSet<Token>,
            state: &State,
        ) -> BTreeSet<Token> {
            let mut common = None;
            if let FutureResult::Cases(cases) = result {
                for case in cases {
                    let mut branch = BTreeSet::new();
                    if let Some(completion) = &case.completion
                        && completion.active
                    {
                        for (identity, object) in &completion.objects {
                            if objects.contains(identity)
                                && identity.observable()
                                && !completion.tainted.contains(identity)
                            {
                                for value in object.fields.values() {
                                    branch.extend(state.freeze(value).guaranteed());
                                }
                            }
                        }
                    }
                    branch.extend(snapshots(&case.result, objects, state));
                    intersect(&mut common, branch);
                }
            }
            common.unwrap_or_default()
        }
        let mut objects = BTreeSet::new();
        let mut futures = BTreeSet::new();
        visible(value, &mut objects, &mut futures);
        let mut accepted = BTreeSet::new();
        fn visit(
            value: &Tracked,
            objects: &BTreeSet<Token>,
            futures: &BTreeSet<Token>,
            state: &State,
            accepted: &mut BTreeSet<Token>,
        ) {
            if let Tracked::Future { token, result, .. } = value
                && futures.contains(token)
                && state
                    .mutations
                    .get(token)
                    .is_some_and(|mutation| mutation.tainted.is_empty())
            {
                accepted.extend(snapshots(result, objects, state));
            }
            let mut value = value.clone();
            value.map_children(&mut |child| visit(child, objects, futures, state, accepted));
        }
        visit(value, &objects, &futures, self, &mut accepted);
        accepted
    }
    pub fn prepare_mutation_return(&self, value: &mut Tracked) {
        if let Tracked::Future { token, result, .. } = value {
            fn prepare(result: &mut FutureResult, mutation: Option<&Mutation>) {
                if let FutureResult::Cases(cases) = result {
                    for case in cases {
                        if let Some(completion) = &mut case.completion {
                            completion.active = mutation.is_some();
                            if let Some(mutation) = mutation {
                                completion.tainted.extend(mutation.tainted.clone());
                            }
                        }
                        prepare(&mut case.result, mutation);
                    }
                }
            }
            prepare(result, self.mutations.get(token));
        }
        value.map_children(&mut |child| self.prepare_mutation_return(child));
    }
    pub fn taint_mutations(&mut self, object: Option<&Token>) {
        for mutation in self.mutations.values_mut() {
            for candidate in &mutation.objects {
                if object.is_none_or(|object| object == candidate) {
                    mutation.tainted.insert(candidate.clone());
                }
            }
        }
    }
    pub fn begin_mutation(&mut self, owner: Token, objects: BTreeSet<Token>) {
        let overlap: BTreeSet<_> = objects.intersection(&self.async_mutable).cloned().collect();
        for object in &objects {
            self.taint_mutations(Some(object));
        }
        self.async_mutable.extend(objects.clone());
        self.mutations.insert(
            owner,
            Mutation {
                objects,
                tainted: overlap,
            },
        );
    }
    pub fn finish_mutation(&mut self, owner: &Token, completion: Option<&Completion>) {
        if !owner.observable() {
            return;
        }
        let Some(mutation) = self.mutations.remove(owner) else {
            return;
        };
        self.async_mutable = self.sticky_mutable.clone();
        for mutation in self.mutations.values() {
            self.async_mutable.extend(mutation.objects.clone());
        }
        if let Some(completion) = completion {
            for (identity, object) in &completion.objects {
                if mutation.tainted.contains(identity)
                    || completion.tainted.contains(identity)
                    || self.async_mutable.contains(identity)
                    || !identity.observable()
                {
                    continue;
                }
                self.objects.insert(identity.clone(), object.clone());
                if let Token::Input(base) = identity {
                    for (name, value) in &object.fields {
                        let mut field = base.clone();
                        field
                            .path
                            .push(Access::Field(object.ty.clone(), name.clone()));
                        self.stores.insert(field, value.clone());
                    }
                }
            }
            if mutation.tainted.is_empty()
                && completion.tainted.is_empty()
                && mutation.objects.is_disjoint(&self.async_mutable)
            {
                for token in &completion.observed {
                    self.observe(token);
                }
            }
        }
    }
    pub fn mutation_tokens(&self) -> BTreeSet<Token> {
        self.mutations.keys().cloned().collect()
    }
    pub fn age_mutation_token(&mut self, current: &Token, old: &Token) {
        if let Some(mutation) = self.mutations.remove(current) {
            self.mutations
                .entry(old.clone())
                .and_modify(|previous| {
                    previous.objects.extend(mutation.objects.clone());
                    previous.tainted.extend(mutation.tainted.clone());
                })
                .or_insert(mutation);
        }
    }
    pub fn rename_mutation_object(&mut self, current: &Token, old: &Token) {
        for mutation in self.mutations.values_mut() {
            if mutation.objects.remove(current) {
                mutation.objects.insert(old.clone());
            }
            if mutation.tainted.remove(current) {
                mutation.tainted.insert(old.clone());
            }
        }
        if self.sticky_mutable.remove(current) {
            self.sticky_mutable.insert(old.clone());
        }
    }
    pub fn join_mutations(&mut self, other: &Self) {
        self.sticky_mutable.extend(other.sticky_mutable.clone());
        self.unknown_async_mutation |= other.unknown_async_mutation;
        for (owner, mutation) in &other.mutations {
            self.mutations
                .entry(owner.clone())
                .and_modify(|previous| {
                    previous.objects.extend(mutation.objects.clone());
                    previous.tainted.extend(mutation.tainted.clone());
                })
                .or_insert_with(|| mutation.clone());
        }
    }
}
fn intersect(common: &mut Option<BTreeSet<Token>>, branch: BTreeSet<Token>) {
    if let Some(common) = common {
        common.retain(|token| branch.contains(token));
    } else {
        *common = Some(branch);
    }
}
pub(super) fn public_futures(value: &Tracked) -> BTreeSet<Token> {
    fn outcome(result: &FutureResult) -> BTreeSet<Token> {
        match result {
            FutureResult::Success(value) => public_futures(value),
            FutureResult::Cases(cases) => {
                let mut common = None;
                for case in cases {
                    intersect(&mut common, outcome(&case.result));
                }
                common.unwrap_or_default()
            }
            _ => BTreeSet::new(),
        }
    }
    match value {
        Tracked::Future { token, result, .. } => {
            let mut handles = outcome(result);
            handles.insert(token.clone());
            handles
        }
        Tracked::Record { fields, .. }
        | Tracked::Reference {
            snapshot: Some(fields),
            ..
        } => fields.values().flat_map(public_futures).collect(),
        Tracked::List(values)
        | Tracked::Batch(values)
        | Tracked::Variant { fields: values, .. } => {
            values.iter().flat_map(public_futures).collect()
        }
        Tracked::Map(entries) => entries
            .iter()
            .flat_map(|(_, value)| public_futures(value))
            .collect(),
        Tracked::Choice(values) => {
            let mut common = None;
            for value in values {
                intersect(&mut common, public_futures(value));
            }
            common.unwrap_or_default()
        }
        _ => BTreeSet::new(),
    }
}
impl Engine<'_> {
    pub fn mutation_identity(&self, value: &Tracked) -> Option<Token> {
        match value {
            Tracked::Reference { identity, .. } => Some(identity.clone()),
            Tracked::Input { parameter, .. } => Some(Token::Input(parameter.clone())),
            _ => None,
        }
    }
    pub fn mutation_targets(
        &self,
        contract: &Contract,
        values: &[Tracked],
        state: &mut State,
    ) -> Option<BTreeSet<Token>> {
        if contract.disrupts_observers || contract.mutates_async || contract.exits.is_empty() {
            return None;
        }
        // Per-parameter summaries do not retain the order between stores
        // through different aliases. Do not publish a guessed final value.
        let mut identities = BTreeSet::new();
        fn distinct(value: &Tracked, identities: &mut BTreeSet<Token>) -> bool {
            if let Tracked::Reference { identity, .. } = value
                && !identities.insert(identity.clone())
            {
                return false;
            }
            let mut valid = true;
            value.clone().map_children(&mut |value| {
                valid &= distinct(value, identities);
            });
            valid
        }
        if !values
            .iter()
            .all(|value| distinct(&state.freeze(value), &mut identities))
        {
            return None;
        }
        let mut targets = BTreeSet::new();
        for exit in &contract.exits {
            for field in exit.stores.keys() {
                let mut base = field.clone();
                base.path.pop()?;
                if !self.stable_projection(&base) {
                    return None;
                }
                let value = self.resolve_parameter(&base, values, Value::Plain, false, state);
                let identity = match &value {
                    Tracked::Reference { identity, .. } => identity.clone(),
                    Tracked::Input {
                        parameter, effect, ..
                    } => {
                        let ty = effect.nominal()?;
                        let schema = self.checked.source_types.get(ty)?;
                        if schema.kind != "state" {
                            return None;
                        }
                        let identity = Token::Input(parameter.clone());
                        if !state.objects.contains_key(&identity) {
                            let fields = schema
                                .fields
                                .iter()
                                .map(|(name, field_ty)| {
                                    (
                                        name.clone(),
                                        self.read_field(state, &value, ty, name, field_ty),
                                    )
                                })
                                .collect();
                            state.objects.insert(
                                identity.clone(),
                                objects::Object {
                                    ty: ty.to_owned(),
                                    fields,
                                },
                            );
                        }
                        identity
                    }
                    _ => return None,
                };
                if !identity.observable() || !state.objects.contains_key(&identity) {
                    return None;
                }
                targets.insert(identity);
            }
            for input in &exit.observed {
                if !self.stable_projection(input)
                    && let Some(objects) = self.observer_objects(input, values, state)
                {
                    targets.extend(objects);
                }
            }
        }
        (!targets.is_empty()).then_some(targets)
    }
    pub fn obscure_mutation_targets(&self, state: &mut State, targets: &BTreeSet<Token>) {
        for identity in targets {
            state.invalidate_watches(Some(identity));
            if let Some(object) = state.objects.get_mut(identity) {
                for (name, value) in &mut object.fields {
                    let ty = self.checked.source_types[&object.ty]
                        .fields
                        .iter()
                        .find(|(field, _)| field == name)
                        .map(|(_, ty)| ty)
                        .expect("checked class field");
                    *value = Tracked::unknown(self.typed(ty), value.tokens());
                    if let Token::Input(base) = identity {
                        let mut field = base.clone();
                        field
                            .path
                            .push(Access::Field(object.ty.clone(), name.clone()));
                        state.stores.insert(field, value.clone());
                        state.mutable_inputs = true;
                    }
                }
            }
        }
    }
}
