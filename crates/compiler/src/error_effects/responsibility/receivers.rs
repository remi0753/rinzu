//! Provisional observation is not completion: cancelling a receiver before it
//! runs restores its producers. Receipts survive ordinary function summaries.
use super::*;
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Receiver {
    pub origin: Site,
    pub producers: BTreeMap<Token, Option<Obligation>>,
    pub completes: BTreeSet<Token>,
    pub covers: BTreeSet<Token>,
    pub parents: BTreeSet<Token>,
    pub valid: bool,
    pub active: bool,
}
impl Receiver {
    pub fn map_tokens(&mut self, visit: &mut impl FnMut(&Token) -> Token) {
        self.producers = std::mem::take(&mut self.producers)
            .into_iter()
            .map(|(token, value)| (visit(&token), value))
            .collect();
        self.completes = std::mem::take(&mut self.completes)
            .iter()
            .map(&mut *visit)
            .collect();
        self.covers = std::mem::take(&mut self.covers)
            .iter()
            .map(&mut *visit)
            .collect();
        self.parents = std::mem::take(&mut self.parents)
            .iter()
            .map(visit)
            .collect();
    }
    pub fn merge(&mut self, other: Self) {
        self.valid &= other.valid;
        self.active |= other.active;
        self.covers.retain(|token| other.covers.contains(token));
        self.completes
            .retain(|token| other.completes.contains(token));
        self.parents.extend(other.parents);
        for (token, receipt) in other.producers {
            self.producers
                .entry(token)
                .and_modify(|left| {
                    if let (Some(left), Some(right)) = (left, &receipt) {
                        left.errors.extend(right.errors.clone());
                    }
                })
                .or_insert(receipt);
        }
    }
}
pub(super) fn canonical(token: &Token) -> Token {
    if let Token::All(inner) | Token::Population(inner) = token {
        canonical(inner)
    } else {
        token.clone()
    }
}
pub(super) fn normalize(token: &Token, slots: &mut BTreeMap<Token, u32>) -> Token {
    if let Token::Population(inner) = token {
        return Token::Population(Box::new(normalize(inner, slots)));
    }
    if let Token::All(inner) = token {
        return Token::All(Box::new(normalize(inner, slots)));
    }
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
}
impl State {
    pub fn provisional_observe(&mut self, token: &Token) {
        if let Token::Iteration(at, path) = token {
            self.iterated.insert((*at, path.clone()));
            return;
        }
        let full = matches!(token, Token::All(_) | Token::Population(_));
        let token = canonical(token);
        if full || token.observable() {
            self.pending.remove(&token);
            if let Token::Input(input) = token {
                self.observed.insert(input);
            }
        }
    }
    // Identity reuse is a membership query. Building the complete receiver
    // population for each instantiated output makes large call summaries
    // quadratic in allocated/cloned token sets.
    pub fn has_receiver_token(&self, token: &Token) -> bool {
        self.receiver_controls.contains(token)
            || self.receivers.iter().any(|(owner, receiver)| {
                owner == token
                    || receiver.producers.contains_key(token)
                    || receiver.parents.contains(token)
            })
    }
    pub fn receiver_tokens(&self) -> BTreeSet<Token> {
        self.receivers
            .iter()
            .flat_map(|(owner, receiver)| {
                std::iter::once(owner.clone())
                    .chain(receiver.producers.keys().cloned())
                    .chain(receiver.parents.iter().cloned())
            })
            .chain(self.receiver_controls.iter().cloned())
            .collect()
    }
    pub fn forget_received_producer(&mut self, token: &Token) {
        for receiver in self.receivers.values_mut() {
            receiver.producers.remove(token);
        }
        self.receivers
            .retain(|_, receiver| !receiver.producers.is_empty());
    }
    pub fn add_receiver(
        &mut self,
        initial: &State,
        owner: Token,
        observed: &BTreeSet<Token>,
        completes: &BTreeSet<Token>,
        covers: &BTreeSet<Token>,
    ) {
        let mut receiver = Receiver {
            producers: BTreeMap::new(),
            completes: completes.clone(),
            covers: covers.iter().map(canonical).collect(),
            parents: BTreeSet::new(),
            valid: true,
            active: true,
            origin: match &owner {
                Token::Local(at, ..) => *at,
                _ => (0, 0, 0, 0, 0),
            },
        };
        for token in observed {
            let token = canonical(token);
            let prior = initial
                .receivers
                .values()
                .find_map(|receiver| receiver.producers.get(&token))
                .cloned();
            let incoming = matches!(&token, Token::Iteration(..))
                || matches!(&token,Token::Input(input) if !initial.observed.contains(input));
            if let Some(receipt) = initial
                .pending
                .get(&token)
                .cloned()
                .map(Some)
                .or(prior)
                .or_else(|| (incoming || initial.receivers.contains_key(&token)).then_some(None))
            {
                receiver.producers.insert(token, receipt);
            }
        }
        if !receiver.producers.is_empty() {
            self.receivers
                .entry(owner)
                .and_modify(|prior| prior.merge(receiver.clone()))
                .or_insert(receiver);
        }
    }
    pub fn cancel_receivers(&mut self, target: Option<&Token>) {
        self.invalidate_receivers(target, false);
    }
    pub fn invalidate_receiver_field(&mut self, owner: &Token) {
        self.invalidate_receivers(Some(owner), true);
    }
    fn invalidate_receivers(&mut self, target: Option<&Token>, field: bool) {
        let target = target.map(canonical);
        let mut restore = BTreeMap::new();
        for (owner, receiver) in &mut self.receivers {
            if receiver.active
                && target
                    .as_ref()
                    .is_none_or(|target| owner == target || receiver.parents.contains(target))
            {
                receiver.valid = false;
                restore.extend(receiver.producers.clone());
            }
        }
        for (producer, obligation) in restore {
            if field
                && (self
                    .receivers
                    .values()
                    .any(|receiver| receiver.valid && receiver.covers.contains(&producer))
                    || self
                        .watches
                        .values()
                        .any(|watch| watch.valid && watch.producers.contains_key(&producer)))
            {
                continue;
            }
            for receiver in self.receivers.values_mut() {
                if !receiver.active && receiver.producers.contains_key(&producer) {
                    receiver.valid = false;
                }
            }
            if let Some(obligation) = obligation {
                self.pending
                    .entry(producer.clone())
                    .and_modify(|left| left.errors.extend(obligation.errors.clone()))
                    .or_insert(obligation);
            }
            if let Token::Input(input) = producer {
                self.observed.remove(&input);
            }
        }
    }
    pub fn receiver_allows(&self, owner: &Token, producer: &Token) -> bool {
        self.receivers
            .get(&canonical(owner))
            .is_none_or(|receiver| {
                receiver.valid || !receiver.producers.contains_key(&canonical(producer))
            })
    }
    pub fn complete_receiver(&mut self, owner: &Token) {
        fn finish(state: &mut State, owner: &Token, seen: &mut BTreeSet<Token>) {
            let all = matches!(owner, Token::Population(_));
            let owner = canonical(owner);
            if !all && !owner.observable() || !seen.insert(owner.clone()) {
                return;
            }
            for receiver in state.receivers.values_mut() {
                receiver.parents.remove(&owner);
            }
            let Some(receiver) = state
                .receivers
                .get(&owner)
                .filter(|receiver| receiver.valid && receiver.active)
                .cloned()
            else {
                return;
            };
            for producer in receiver.completes {
                if !receiver.producers.contains_key(&canonical(&producer)) {
                    continue;
                }
                let producer = if all
                    && receiver.covers.contains(&canonical(&producer))
                    && !canonical(&producer).observable()
                {
                    Token::Population(Box::new(canonical(&producer)))
                } else {
                    producer
                };
                finish(state, &producer, seen);
                state.observe(&producer);
            }
            if let Some(receiver) = state.receivers.get_mut(&owner) {
                receiver.active = false;
            }
        }
        finish(self, owner, &mut BTreeSet::new());
    }
    pub fn age_receiver_token(&mut self, current: &Token, old: &Token) {
        self.age_receivers(&BTreeMap::from([(current.clone(), old.clone())]));
    }
    pub fn receiver_population(&self) -> BTreeSet<Token> {
        self.pending
            .keys()
            .cloned()
            .chain(
                self.receivers
                    .values()
                    .flat_map(|receiver| receiver.producers.keys().cloned()),
            )
            .collect()
    }
    pub fn task_population(&self) -> BTreeSet<Token> {
        self.receiver_population()
            .into_iter()
            .chain(
                self.receivers
                    .iter()
                    .filter(|(_, receiver)| receiver.active)
                    .map(|(owner, _)| owner.clone()),
            )
            .chain(
                self.receivers
                    .values()
                    .flat_map(|receiver| receiver.parents.iter().cloned()),
            )
            .chain(self.mutations.keys().cloned())
            .collect()
    }
    pub fn age_receivers(&mut self, renames: &BTreeMap<Token, Token>) {
        fn rename(token: &Token, renames: &BTreeMap<Token, Token>) -> Token {
            if let Token::All(inner) = token {
                return Token::All(Box::new(rename(inner, renames)));
            }
            if let Token::Population(inner) = token {
                return Token::Population(Box::new(rename(inner, renames)));
            }
            renames.get(token).cloned().unwrap_or_else(|| token.clone())
        }
        let population = self.receiver_population();
        let mut groups: BTreeMap<Token, Vec<Receiver>> = BTreeMap::new();
        for (owner, receiver) in std::mem::take(&mut self.receivers) {
            groups
                .entry(rename(&owner, renames))
                .or_default()
                .push(receiver);
        }
        for (owner, group) in groups {
            let candidates: BTreeSet<_> = group
                .iter()
                .flat_map(|receiver| receiver.covers.iter().map(|token| rename(token, renames)))
                .collect();
            let covers = candidates
                .into_iter()
                .filter(|candidate| {
                    population
                        .iter()
                        .filter(|source| rename(source, renames) == *candidate)
                        .all(|source| {
                            group
                                .iter()
                                .any(|receiver| receiver.covers.contains(source))
                        })
                })
                .collect();
            let mut merged: Option<Receiver> = None;
            for mut receiver in group {
                receiver.map_tokens(&mut |token| rename(token, renames));
                if let Some(merged) = &mut merged {
                    merged.merge(receiver);
                } else {
                    merged = Some(receiver);
                }
            }
            let mut merged = merged.expect("nonempty receiver group");
            merged.covers = covers;
            self.receivers.insert(owner, merged);
        }
        self.receiver_controls = std::mem::take(&mut self.receiver_controls)
            .iter()
            .map(|token| rename(token, renames))
            .collect();
    }
    pub fn join_receivers(&mut self, other: &State) {
        let left_population = self.receiver_population();
        let right_population = other.receiver_population();
        let owners: BTreeSet<_> = self
            .receivers
            .keys()
            .chain(other.receivers.keys())
            .cloned()
            .collect();
        for owner in owners {
            let left = self.receivers.get(&owner);
            let right = other.receivers.get(&owner);
            let candidates: BTreeSet<_> = left
                .into_iter()
                .chain(right)
                .flat_map(|receiver| receiver.covers.iter().cloned())
                .collect();
            let covers = candidates
                .into_iter()
                .filter(|producer| {
                    (!left_population.contains(producer)
                        || left.is_some_and(|receiver| receiver.covers.contains(producer)))
                        && (!right_population.contains(producer)
                            || right.is_some_and(|receiver| receiver.covers.contains(producer)))
                })
                .collect();
            let mut receiver = match (left, right) {
                (Some(left), Some(right)) => {
                    let mut receiver = left.clone();
                    receiver.merge(right.clone());
                    receiver
                }
                (Some(receiver), None) | (None, Some(receiver)) => receiver.clone(),
                _ => unreachable!(),
            };
            receiver.covers = covers;
            self.receivers.insert(owner, receiver);
        }
        self.receiver_controls
            .extend(other.receiver_controls.clone());
        self.receiver_scope_control |= other.receiver_scope_control;
    }
    pub fn direct_observed(&self) -> BTreeSet<Parameter> {
        self.observed
            .iter()
            .filter(|input| {
                !self.receivers.values().any(|receiver| {
                    receiver
                        .producers
                        .contains_key(&Token::Input((*input).clone()))
                })
            })
            .cloned()
            .collect()
    }
    pub fn fold_receiver_summary(
        &self,
        public: &BTreeSet<Token>,
        cyclic: bool,
    ) -> (BTreeMap<Token, Receiver>, BTreeSet<Token>) {
        if !cyclic {
            return (self.receivers.clone(), self.receiver_controls.clone());
        }
        let mut by_origin: BTreeMap<Site, BTreeSet<Token>> = BTreeMap::new();
        let mut producers: BTreeMap<Site, BTreeSet<Token>> = BTreeMap::new();
        let mut ancestors: BTreeMap<Site, BTreeSet<Token>> = BTreeMap::new();
        fn root_site(token: &Token) -> Option<Site> {
            match token {
                Token::Local(at, ..) => Some(*at),
                Token::Family(inner) | Token::All(inner) | Token::Population(inner) => {
                    root_site(inner)
                }
                _ => None,
            }
        }
        for (owner, receiver) in &self.receivers {
            if !public.contains(owner) {
                by_origin
                    .entry(receiver.origin)
                    .or_default()
                    .insert(owner.clone());
            }
            for (token, receipt) in &receiver.producers {
                if !public.contains(token)
                    && let Some(receipt) = receipt
                    && matches!(token, Token::Local(..) | Token::Family(_))
                {
                    producers
                        .entry(receipt.origin)
                        .or_default()
                        .insert(token.clone());
                }
            }
            for token in &receiver.parents {
                if !public.contains(token)
                    && let Some(at) = root_site(token)
                {
                    ancestors.entry(at).or_default().insert(token.clone());
                }
            }
        }
        let mut renamed = BTreeMap::new();
        let mut folded_producers = BTreeSet::new();
        for (origin, tokens) in by_origin {
            if tokens.len() > 1 {
                for token in tokens {
                    renamed.insert(token, Token::Local(origin, u32::MAX, true));
                }
            }
        }
        for (origin, tokens) in producers {
            if tokens.len() > 1 {
                for token in tokens {
                    folded_producers.insert(token.clone());
                    renamed.insert(token, Token::Local(origin, u32::MAX - 1, true));
                }
            }
        }
        for (origin, tokens) in ancestors {
            if tokens.len() > 1 {
                for token in tokens {
                    if let std::collections::btree_map::Entry::Vacant(entry) = renamed.entry(token)
                    {
                        folded_producers.insert(entry.key().clone());
                        entry.insert(Token::Local(origin, u32::MAX - 2, true));
                    }
                }
            }
        }
        let rename = |token: &Token| {
            if let Token::All(inner) | Token::Population(inner) = token {
                let renamed = renamed
                    .get(inner.as_ref())
                    .cloned()
                    .unwrap_or_else(|| inner.as_ref().clone());
                if folded_producers.contains(inner.as_ref()) {
                    renamed
                } else if matches!(token, Token::Population(_)) {
                    Token::Population(Box::new(renamed))
                } else {
                    Token::All(Box::new(renamed))
                }
            } else {
                renamed.get(token).cloned().unwrap_or_else(|| token.clone())
            }
        };
        let mut result: BTreeMap<Token, Receiver> = BTreeMap::new();
        for (owner, receiver) in &self.receivers {
            let mut receiver = receiver.clone();
            receiver
                .covers
                .retain(|token| !folded_producers.contains(token));
            receiver.map_tokens(&mut |token| rename(token));
            result
                .entry(rename(owner))
                .and_modify(|prior| prior.merge(receiver.clone()))
                .or_insert(receiver);
        }
        (result, self.receiver_controls.iter().map(rename).collect())
    }
}
impl Engine<'_> {
    pub fn callback_scope_control(&self, value: &Value) -> bool {
        let controls = |name: &str| {
            self.contracts.get(name).is_some_and(|contract| {
                contract
                    .exits
                    .iter()
                    .any(|exit| exit.receiver_scope_control)
            })
        };
        match value {
            Value::Callable(names) => names.iter().any(|name| controls(name)),
            Value::Aggregate { ty, .. } | Value::Opaque(ty) => {
                self.private_environment(ty) && {
                    let prefix = format!("{ty}_callback_");
                    self.contracts.keys().any(|name| {
                        name.starts_with(&prefix) && {
                            let Some(function) = self
                                .checked
                                .functions
                                .iter()
                                .find(|function| function.declaration.name == *name)
                            else {
                                return false;
                            };
                            let mut effects: Vec<_> = function
                                .parameter_types
                                .iter()
                                .map(|ty| self.typed(ty))
                                .collect();
                            if let Some(first) = effects.first_mut() {
                                *first = value.clone();
                            }
                            self.contracts[name].exits.iter().any(|exit| {
                                exit.receiver_scope_control
                                    && !substitution::language(
                                        &Language::from([language::Word {
                                            prefix: Vec::new(),
                                            longer: false,
                                            guards: exit.guards.clone(),
                                        }]),
                                        name,
                                        &effects,
                                        self.bound,
                                    )
                                    .is_empty()
                            })
                        }
                    })
                }
            }
            Value::Parameter { fallback, .. }
            | Value::Unbound(fallback)
            | Value::Conditional {
                value: fallback, ..
            } => self.callback_scope_control(fallback),
            Value::Union(values) => values
                .iter()
                .any(|value| self.callback_scope_control(value)),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reusing_a_producer_identity_keeps_the_old_receivers_cancellation_receipt() {
        let at = (1, 10, 20, 0, 0);
        let current = Token::Local(at, 0, false);
        let old = Token::Local(at, 0, true);
        let owner = Token::Local((1, 30, 40, 0, 0), 0, false);
        let errors = Language::from([language::Word {
            prefix: vec![Tag::Named("Failed".into())],
            longer: false,
            guards: BTreeMap::new(),
        }]);
        let mut state = State::default();
        state.receivers.insert(
            owner,
            Receiver {
                origin: at,
                producers: BTreeMap::from([(
                    current.clone(),
                    Some(Obligation {
                        origin: at,
                        errors: errors.clone(),
                    }),
                )]),
                completes: BTreeSet::from([current.clone()]),
                covers: BTreeSet::from([current.clone()]),
                parents: BTreeSet::new(),
                valid: true,
                active: true,
            },
        );
        assert_eq!(state.fresh(at, 0), current);
        state.cancel_receivers(None);
        assert_eq!(
            state.pending.get(&old).map(|obligation| &obligation.errors),
            Some(&errors)
        );
        assert!(!state.pending.contains_key(&current));
    }
}
