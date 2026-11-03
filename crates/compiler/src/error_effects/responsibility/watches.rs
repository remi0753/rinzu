//! A readonly task may observe a mutable field only while the snapshot remains
//! valid. A later store restores responsibility rather than assuming task order.
use super::*;
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct Watch {
    pub objects: BTreeSet<Token>,
    pub producers: BTreeMap<Token, Option<Obligation>>,
    pub valid: bool,
}
impl Watch {
    fn merge(&mut self, other: Self) {
        self.objects.extend(other.objects);
        self.valid &= other.valid;
        for (token, obligation) in other.producers {
            self.producers
                .entry(token)
                .and_modify(|left| {
                    if let (Some(left), Some(right)) = (left, &obligation) {
                        left.errors.extend(right.errors.clone());
                    }
                })
                .or_insert(obligation);
        }
    }
}
impl State {
    pub fn sanitize_watched_observations(&self, value: &mut Tracked) {
        if let Tracked::Future { token, result, .. } = value
            && let Some(receiver) = self.receivers.get(token).filter(|receiver| !receiver.valid)
        {
            fn remove_receiver(result: &mut FutureResult, denied: &receivers::Receiver) {
                if let FutureResult::Cases(cases) = result {
                    for case in cases {
                        case.observed.retain(|token| {
                            !denied.producers.contains_key(&receivers::canonical(token))
                        });
                        remove_receiver(&mut case.result, denied);
                    }
                }
            }
            remove_receiver(result, receiver);
        }
        if let Tracked::Future { token, result, .. } = value
            && let Some(watch) = self.watches.get(token).filter(|watch| !watch.valid)
        {
            fn remove(result: &mut FutureResult, denied: &BTreeMap<Token, Option<Obligation>>) {
                if let FutureResult::Cases(cases) = result {
                    for case in cases {
                        case.observed
                            .retain(|producer| !denied.contains_key(producer));
                        remove(&mut case.result, denied);
                    }
                }
            }
            remove(result, &watch.producers);
        }
        value.map_children(&mut |child| self.sanitize_watched_observations(child));
    }
    pub fn watch_tokens(&self) -> BTreeSet<Token> {
        self.watches
            .iter()
            .flat_map(|(owner, watch)| {
                std::iter::once(owner.clone()).chain(watch.producers.keys().cloned())
            })
            .collect()
    }
    pub fn forget_watched_producer(&mut self, producer: &Token) {
        for watch in self.watches.values_mut() {
            watch.producers.remove(producer);
        }
    }
    pub fn invalidate_watches(&mut self, object: Option<&Token>) {
        let mut restore = BTreeMap::new();
        let mut receivers = Vec::new();
        for (owner, watch) in &mut self.watches {
            if object.is_none_or(|object| watch.objects.contains(object)) {
                receivers.push(owner.clone());
                watch.valid = false;
                for (producer, obligation) in &watch.producers {
                    restore
                        .entry(producer.clone())
                        .or_insert_with(|| obligation.clone());
                }
            }
        }
        for owner in receivers {
            self.invalidate_receiver_field(&owner);
        }
        for (producer, obligation) in restore {
            if self
                .receivers
                .values()
                .any(|receiver| receiver.valid && receiver.covers.contains(&producer))
            {
                continue;
            }
            if self
                .watches
                .values()
                .any(|watch| watch.valid && watch.producers.contains_key(&producer))
            {
                continue;
            }
            if let Some(obligation) = obligation {
                self.pending
                    .entry(producer.clone())
                    .and_modify(|previous| previous.errors.extend(obligation.errors.clone()))
                    .or_insert(obligation);
            }
            if let Token::Input(parameter) = producer {
                self.observed.remove(&parameter);
            }
        }
    }
    pub fn complete_watch(&mut self, owner: &Token) {
        if !owner.observable() {
            return;
        }
        self.complete_receiver(owner);
        if let Some(watch) = self.watches.get(owner).filter(|watch| watch.valid).cloned() {
            self.watches.remove(owner);
            let producers = watch.producers.keys().cloned().collect::<Vec<_>>();
            for producer in producers {
                self.observe_after_watch(owner, &producer);
            }
        }
    }
    pub fn observe_after_watch(&mut self, owner: &Token, producer: &Token) {
        if !self.receiver_allows(owner, producer) {
            return;
        }
        if self
            .receivers
            .get(&receivers::canonical(owner))
            .is_some_and(|receiver| {
                receiver
                    .producers
                    .contains_key(&receivers::canonical(producer))
                    && !receiver
                        .completes
                        .iter()
                        .any(|token| receivers::canonical(token) == receivers::canonical(producer))
            })
        {
            self.provisional_observe(producer);
            return;
        }
        if self
            .watches
            .get(owner)
            .is_some_and(|watch| !watch.valid && watch.producers.contains_key(producer))
        {
            return;
        }
        self.complete_receiver(producer);
        self.observe(producer);
    }
    pub fn rename_watch_object(&mut self, current: &Token, old: &Token) {
        for watch in self.watches.values_mut() {
            if watch.objects.remove(current) {
                watch.objects.insert(old.clone());
            }
        }
    }
    pub fn age_watch_token(&mut self, current: &Token, old: &Token) {
        for watch in self.watches.values_mut() {
            if let Some(obligation) = watch.producers.remove(current) {
                watch
                    .producers
                    .entry(old.clone())
                    .and_modify(|left| {
                        if let (Some(left), Some(right)) = (left, &obligation) {
                            left.errors.extend(right.errors.clone());
                        }
                    })
                    .or_insert(obligation);
            }
        }
        if let Some(watch) = self.watches.remove(current) {
            self.watches
                .entry(old.clone())
                .and_modify(|left| left.merge(watch.clone()))
                .or_insert(watch);
        }
    }
    pub fn join_watches(&mut self, other: &Self) {
        for (owner, watch) in &mut self.watches {
            if !other.watches.contains_key(owner) {
                watch.valid = false;
            }
        }
        for (owner, watch) in &other.watches {
            if let Some(left) = self.watches.get_mut(owner) {
                left.merge(watch.clone());
            } else {
                let mut watch = watch.clone();
                watch.valid = false;
                self.watches.insert(owner.clone(), watch);
            }
        }
    }
    pub fn add_watch(
        &mut self,
        initial: &Self,
        owner: Token,
        candidates: &BTreeMap<Token, BTreeSet<Token>>,
        definitive: &BTreeSet<Token>,
        at: Site,
    ) {
        let mut watch = Watch {
            objects: BTreeSet::new(),
            producers: BTreeMap::new(),
            valid: true,
        };
        for (producer, objects) in candidates {
            if definitive.contains(producer)
                || !producer.observable()
                || matches!(producer, Token::All(_) | Token::Iteration(..))
            {
                continue;
            }
            let covered = !self.pending.contains_key(producer)
                && match producer {
                    Token::Input(parameter) => self.observed.contains(parameter),
                    _ => true,
                };
            if !covered {
                continue;
            }
            let prior = initial
                .watches
                .values()
                .find(|watch| watch.valid && watch.producers.contains_key(producer));
            let incoming =
                matches!(producer,Token::Input(parameter) if !initial.observed.contains(parameter));
            if !initial.pending.contains_key(producer) && prior.is_none() && !incoming {
                continue;
            }
            let obligation = initial
                .pending
                .get(producer)
                .cloned()
                .or_else(|| prior.and_then(|watch| watch.producers[producer].clone()));
            if producer == &owner {
                if let Some(obligation) = obligation {
                    self.requirements.push(Requirement {
                        kind: RequirementKind::Observation,
                        origin: obligation.origin,
                        exit: at,
                        errors: obligation.errors,
                    });
                }
                continue;
            }
            // invoke/defer provisionally used observe; retain older independent
            // leases, which definitive observation would otherwise remove.
            for (previous, prior) in &initial.watches {
                if let Some(receipt) = prior.producers.get(producer) {
                    self.watches
                        .entry(previous.clone())
                        .or_insert_with(|| prior.clone())
                        .producers
                        .insert(producer.clone(), receipt.clone());
                }
            }
            watch.objects.extend(objects.clone());
            watch.producers.insert(producer.clone(), obligation);
        }
        if !watch.producers.is_empty() {
            self.watches
                .entry(owner)
                .and_modify(|prior| prior.merge(watch.clone()))
                .or_insert(watch);
        }
    }
}
impl Engine<'_> {
    pub fn observer_objects(
        &self,
        input: &Parameter,
        values: &[Tracked],
        state: &State,
    ) -> Option<BTreeSet<Token>> {
        let mut value = values.get(input.index)?.clone();
        let mut objects = BTreeSet::new();
        for access in &input.path {
            if let Access::Field(owner, name) = access {
                if !self.stable_projection(&Parameter {
                    owner: input.owner.clone(),
                    index: input.index,
                    path: vec![access.clone()],
                }) {
                    let Tracked::Reference { identity, .. } = &value else {
                        return None;
                    };
                    if !identity.observable() {
                        return None;
                    }
                    objects.insert(identity.clone());
                }
                let ty = self
                    .checked
                    .source_types
                    .get(owner)?
                    .fields
                    .iter()
                    .find(|(field, _)| field == name)?
                    .1
                    .clone();
                value = self.read_field(state, &value, owner, name, &ty);
            } else {
                value = value.project(access.clone(), Value::Plain, false);
            }
        }
        (!objects.is_empty()).then_some(objects)
    }
}
