//! Possible membership and complete coverage of pending task families differ.
use super::*;
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct FutureMembers {
    // Map insertion requires a scoped proof of unique keys. This is never a
    // snapshot identity: two widened maps may contain different key sets.
    pub map_origin: Option<Site>,
    pub element: Value,
    pub members: BTreeSet<Token>,
    pub complete: BTreeSet<Token>,
    pub pending: BTreeSet<Token>,
    pub population: BTreeSet<Token>,
    pub population_complete: BTreeSet<Token>,
}
impl FutureMembers {
    pub fn empty_map(at: Site) -> Self {
        Self {
            map_origin: Some(at),
            element: Value::Bottom,
            members: BTreeSet::new(),
            complete: BTreeSet::new(),
            pending: BTreeSet::new(),
            population: BTreeSet::new(),
            population_complete: BTreeSet::new(),
        }
    }
    pub fn from_value(value: &Tracked) -> Option<Self> {
        match value {
            Tracked::Members(members) => Some(members.clone()),
            Tracked::List(values) => {
                let mut result = Self {
                    map_origin: None,
                    element: Value::Bottom,
                    members: BTreeSet::new(),
                    complete: BTreeSet::new(),
                    pending: BTreeSet::new(),
                    population: BTreeSet::new(),
                    population_complete: BTreeSet::new(),
                };
                for value in values {
                    let token = match value {
                        Tracked::Future { token, .. } => token.clone(),
                        Tracked::Input {
                            parameter,
                            future: true,
                            ..
                        } => Token::Input(parameter.clone()),
                        _ => return None,
                    };
                    result.element.join(value.effect());
                    result.members.insert(token.clone());
                    if token.observable() {
                        result.population_complete.insert(token.clone());
                        result.complete.insert(token);
                    }
                }
                Some(result)
            }
            _ => None,
        }
    }
    pub fn append(&mut self, value: &Tracked) -> bool {
        let token = match value {
            Tracked::Future { token, .. } => token.clone(),
            Tracked::Input {
                parameter,
                future: true,
                ..
            } => Token::Input(parameter.clone()),
            _ => return false,
        };
        self.element.join(value.effect());
        self.members.insert(token.clone());
        if token.observable() {
            self.population_complete.insert(token.clone());
            self.complete.insert(token);
        }
        true
    }
    pub fn age(
        &mut self,
        current: &Token,
        old: &Token,
        old_pending: bool,
        current_pending: bool,
        old_present: bool,
        current_present: bool,
    ) {
        let covered_old = !old_present || self.population_complete.contains(old);
        let covered_current = self.population_complete.remove(current);
        if current_present {
            if covered_old && covered_current {
                self.population_complete.insert(old.clone());
            } else {
                self.population_complete.remove(old);
            }
        }
        if self.population.remove(current) {
            self.population.insert(old.clone());
        }
        let covered_old = !old_pending || self.complete.contains(old);
        let covered_current = self.complete.contains(current);
        self.complete.remove(current);
        if current_pending {
            if covered_old && covered_current {
                self.complete.insert(old.clone());
            } else {
                self.complete.remove(old);
            }
        }
        if self.members.remove(current) {
            self.members.insert(old.clone());
        }
        if self.pending.remove(current) {
            self.pending.insert(old.clone());
        }
    }
    pub fn join(
        &mut self,
        other: &Self,
        left_pending: &BTreeSet<Token>,
        right_pending: &BTreeSet<Token>,
        left_population: &BTreeSet<Token>,
        right_population: &BTreeSet<Token>,
    ) {
        self.element.join(other.element.clone());
        let candidates = self
            .population_complete
            .union(&other.population_complete)
            .cloned()
            .collect::<BTreeSet<_>>();
        self.population_complete = candidates
            .into_iter()
            .filter(|token| {
                (!left_population.contains(token) || self.population_complete.contains(token))
                    && (!right_population.contains(token)
                        || other.population_complete.contains(token))
            })
            .collect();
        self.population.extend(other.population.clone());
        self.members.extend(other.members.clone());
        let candidates = self
            .complete
            .union(&other.complete)
            .cloned()
            .collect::<BTreeSet<_>>();
        self.complete = candidates
            .into_iter()
            .filter(|token| {
                (!left_pending.contains(token) || self.complete.contains(token))
                    && (!right_pending.contains(token) || other.complete.contains(token))
            })
            .collect();
        self.pending.extend(other.pending.clone());
    }
    pub fn visit_tokens(&mut self, visit: &mut impl FnMut(&mut Token, &mut bool)) {
        fn remap(tokens: &mut BTreeSet<Token>, visit: &mut impl FnMut(&mut Token, &mut bool)) {
            *tokens = std::mem::take(tokens)
                .into_iter()
                .map(|mut token| {
                    visit(&mut token, &mut false);
                    token
                })
                .collect();
        }
        remap(&mut self.members, visit);
        remap(&mut self.complete, visit);
        remap(&mut self.pending, visit);
        remap(&mut self.population, visit);
        remap(&mut self.population_complete, visit);
    }
}
impl Tracked {
    pub fn age_family(
        &mut self,
        current: &Token,
        old: &Token,
        old_pending: bool,
        current_pending: bool,
        old_present: bool,
        current_present: bool,
    ) {
        if matches!(self, Self::List(_))
            && self
                .tokens()
                .iter()
                .any(|token| token == current || token == old)
            && let Some(members) = FutureMembers::from_value(self)
        {
            *self = Self::Members(members);
        }
        match self {
            Self::CollectionView(elements) => elements.age_family(
                current,
                old,
                old_pending,
                current_pending,
                old_present,
                current_present,
            ),
            Self::Members(members) => members.age(
                current,
                old,
                old_pending,
                current_pending,
                old_present,
                current_present,
            ),
            Self::Record { fields, .. } => {
                for value in fields.values_mut() {
                    value.age_family(
                        current,
                        old,
                        old_pending,
                        current_pending,
                        old_present,
                        current_present,
                    );
                }
            }
            Self::List(values)
            | Self::Batch(values)
            | Self::Choice(values)
            | Self::Variant { fields: values, .. } => {
                for value in values {
                    value.age_family(
                        current,
                        old,
                        old_pending,
                        current_pending,
                        old_present,
                        current_present,
                    );
                }
            }
            Self::Map(entries) => {
                for (_, value) in entries {
                    value.age_family(
                        current,
                        old,
                        old_pending,
                        current_pending,
                        old_present,
                        current_present,
                    );
                }
            }
            Self::Future { result, .. } => result.map_values(&mut |value| {
                value.age_family(
                    current,
                    old,
                    old_pending,
                    current_pending,
                    old_present,
                    current_present,
                )
            }),
            _ => {}
        }
        // Ordinary aliases also age; they never acquire whole-family coverage.
        self.rename(current, old);
    }
    pub fn join_families(
        &mut self,
        other: Self,
        left_pending: &BTreeSet<Token>,
        right_pending: &BTreeSet<Token>,
        left_population: &BTreeSet<Token>,
        right_population: &BTreeSet<Token>,
    ) {
        if *self == other {
            return;
        }
        let map_origin = match (&*self, &other) {
            (Self::Members(members), _) | (_, Self::Members(members)) => members.map_origin,
            _ => None,
        };
        let members = |value: &Self| {
            if let (Some(at), Self::Map(entries)) = (map_origin, value)
                && entries.is_empty()
            {
                Some(FutureMembers::empty_map(at))
            } else {
                FutureMembers::from_value(value)
            }
        };
        if let (Some(mut left), Some(right)) = (members(self), members(&other)) {
            if left.map_origin != right.map_origin {
                self.join(other);
                return;
            }
            left.join(
                &right,
                left_pending,
                right_pending,
                left_population,
                right_population,
            );
            *self = Self::Members(left);
            return;
        }
        match (&mut *self, other) {
            (
                Self::Record { ty, fields },
                Self::Record {
                    ty: rhs,
                    fields: other,
                },
            ) if *ty == rhs && fields.keys().eq(other.keys()) => {
                for (name, value) in fields {
                    value.join_families(
                        other[name].clone(),
                        left_pending,
                        right_pending,
                        left_population,
                        right_population,
                    );
                }
            }
            (Self::List(left), Self::List(right)) if left.len() == right.len() => {
                for (left, right) in left.iter_mut().zip(right) {
                    left.join_families(
                        right,
                        left_pending,
                        right_pending,
                        left_population,
                        right_population,
                    );
                }
            }
            (
                Self::Variant { ty, name, fields },
                Self::Variant {
                    ty: rhs,
                    name: variant,
                    fields: other,
                },
            ) if *ty == rhs && *name == variant && fields.len() == other.len() => {
                for (left, right) in fields.iter_mut().zip(other) {
                    left.join_families(
                        right,
                        left_pending,
                        right_pending,
                        left_population,
                        right_population,
                    );
                }
            }
            (left, right) => left.join(right),
        }
    }
}
