use super::*;

pub(super) type Site = (u32, u32, u32, u32, u32);
pub(super) fn site(range: SourceRange) -> Site {
    (
        range.file_id.0,
        range.start_byte,
        range.end_byte,
        range.instance,
        0,
    )
}
pub(super) fn range(site: Site) -> SourceRange {
    SourceRange {
        file_id: crate::FileId(site.0),
        start_byte: site.1,
        end_byte: site.2,
        instance: site.3,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) enum Key {
    Int(i64),
    String(String),
    Bool(bool),
    Iteration(Site),
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) enum Access {
    Field(String, String),
    Element(String, Option<Key>, Site),
    Each(String),
    MapValues(String),
    Variant(String, String, usize),
    Await(String),
}
impl Access {
    fn effect(&self) -> Projection {
        match self {
            Self::Field(ty, name) => Projection::Field(ty.clone(), name.clone()),
            Self::Element(ty, _, at) => Projection::Element(ty.clone(), *at),
            Self::Each(ty) => Projection::AllElements(ty.clone()),
            Self::MapValues(ty) => Projection::MapValues(ty.clone()),
            Self::Variant(ty, name, index) => Projection::Variant(ty.clone(), name.clone(), *index),
            Self::Await(ty) => Projection::Await(ty.clone()),
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) struct Parameter {
    pub owner: String,
    pub index: usize,
    pub path: Vec<Access>,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) enum Token {
    Input(Parameter),
    Local(Site, u32, bool),
    Output(u32),
    OutputFamily(u32),
    Family(Box<Token>),
    All(Box<Token>),
    Population(Box<Token>),
    Iteration(Site, Vec<Access>),
}
impl Token {
    pub fn observable(&self) -> bool {
        !matches!(
            self,
            Self::Local(_, _, true) | Self::Family(_) | Self::OutputFamily(_)
        )
    }
    pub fn aged(&self) -> Option<Self> {
        match self {
            Self::Local(at, slot, false) => Some(Self::Local(*at, *slot, true)),
            Self::Family(inner) => inner.aged().map(|old| Self::Family(Box::new(old))),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) enum FutureResult {
    Success(Box<Tracked>),
    Failure(language::Word),
    Panic,
    Pending(BTreeSet<Token>),
    // Registering independent tasks does not select their eventual outcomes.
    // Keep the alternatives on the Future and select them at its await sites.
    Cases(Vec<FutureCase>),
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct FutureCase {
    pub guards: language::Guards,
    pub result: FutureResult,
    pub observed: BTreeSet<Token>,
    // Only owned tasks newly reachable in this outcome. A failed producer
    // must not leave obligations belonging exclusively to a successful result.
    pub created: BTreeMap<Token, Obligation>,
    pub completion: Option<mutations::Completion>,
}
impl FutureResult {
    pub fn effect(&self) -> Value {
        match self {
            Self::Panic => Value::PanicFuture,
            Self::Success(value) => Value::Future {
                errors: Language::new(),
                success: true,
                value: Box::new(value.effect()),
            },
            Self::Failure(word) => Value::Future {
                errors: Language::from([word.clone()]),
                success: false,
                value: Box::new(Value::Bottom),
            },
            Self::Pending(held) if held.is_empty() => Value::NeverFuture,
            Self::Pending(_) => Value::Future {
                errors: Language::new(),
                success: true,
                value: Box::new(Value::Plain),
            },
            Self::Cases(cases) => {
                let mut effect = Value::Bottom;
                for case in cases {
                    effect.join(Value::Conditional {
                        guards: case.guards.clone(),
                        value: Box::new(case.result.effect()),
                    });
                }
                effect
            }
        }
    }
    pub fn tokens(&self) -> BTreeSet<Token> {
        match self {
            Self::Success(value) => value.tokens(),
            Self::Pending(held) => held.clone(),
            Self::Cases(cases) => cases
                .iter()
                .flat_map(|case| {
                    case.result
                        .tokens()
                        .into_iter()
                        .chain(case.observed.iter().cloned())
                        .chain(case.created.keys().cloned())
                        .chain(
                            case.completion
                                .iter()
                                .flat_map(mutations::Completion::tokens),
                        )
                })
                .collect(),
            Self::Failure(_) | Self::Panic => BTreeSet::new(),
        }
    }
    pub fn guaranteed(&self) -> BTreeSet<Token> {
        match self {
            Self::Success(value) => value.guaranteed(),
            Self::Pending(held) => held
                .iter()
                .filter(|token| token.observable())
                .cloned()
                .collect(),
            Self::Failure(_) | Self::Panic => BTreeSet::new(),
            Self::Cases(cases) => {
                let created: BTreeSet<_> = cases
                    .iter()
                    .flat_map(|case| case.created.keys().cloned())
                    .collect();
                let mut owned = BTreeSet::new();
                let mut common: Option<BTreeSet<Token>> = None;
                for case in cases {
                    let mut accepted = case.result.guaranteed();
                    accepted.extend(
                        case.observed
                            .iter()
                            .filter(|token| token.observable())
                            .cloned(),
                    );
                    owned.extend(accepted.intersection(&created).cloned());
                    accepted.retain(|token| !created.contains(token));
                    if let Some(common) = &mut common {
                        common.retain(|token| accepted.contains(token));
                    } else {
                        common = Some(accepted);
                    }
                }
                owned.extend(common.unwrap_or_default());
                owned
            }
        }
    }
    pub fn visit_tokens(&mut self, visit: &mut impl FnMut(&mut Token, &mut bool)) {
        match self {
            Self::Success(value) => value.visit_tokens(visit),
            Self::Pending(held) => visit_set(held, visit),
            Self::Cases(cases) => {
                for case in cases {
                    if let Some(completion) = &mut case.completion {
                        completion.visit_tokens(visit);
                    }
                    case.result.visit_tokens(visit);
                    visit_set(&mut case.observed, visit);
                    case.created = std::mem::take(&mut case.created)
                        .into_iter()
                        .map(|(mut token, obligation)| {
                            visit(&mut token, &mut false);
                            (token, obligation)
                        })
                        .collect();
                }
            }
            Self::Failure(_) | Self::Panic => {}
        }
    }
    pub fn map_values(&mut self, visit: &mut impl FnMut(&mut Tracked)) {
        match self {
            Self::Success(value) => visit(value),
            Self::Cases(cases) => {
                for case in cases {
                    case.result.map_values(visit);
                    if let Some(completion) = &mut case.completion {
                        completion.map_values(visit);
                    }
                }
            }
            _ => {}
        }
    }
}
fn visit_set(tokens: &mut BTreeSet<Token>, visit: &mut impl FnMut(&mut Token, &mut bool)) {
    *tokens = std::mem::take(tokens)
        .into_iter()
        .map(|mut token| {
            visit(&mut token, &mut false);
            token
        })
        .collect();
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) enum Tracked {
    Reference {
        identity: Token,
        ty: String,
        snapshot: Option<BTreeMap<String, Tracked>>,
    },
    Data {
        effect: Value,
        key: Option<Key>,
    },
    Input {
        parameter: Parameter,
        effect: Value,
        future: bool,
    },
    Future {
        token: Token,
        result: FutureResult,
        pending: bool,
    },
    Record {
        ty: String,
        fields: BTreeMap<String, Tracked>,
    },
    Variant {
        ty: String,
        name: String,
        fields: Vec<Tracked>,
    },
    List(Vec<Tracked>),
    Members(FutureMembers),
    AllMembers {
        tokens: BTreeSet<Token>,
        population: BTreeSet<Token>,
        effect: Value,
    },
    Map(Vec<(Option<Key>, Tracked)>),
    /// Internal all-elements projection, never a source value or one arbitrary element.
    Batch(Vec<Tracked>),
    /// A representative iteration used only while proving an exhaustive loop.
    Iteration {
        at: Site,
        base: Box<Tracked>,
        path: Vec<Access>,
        effect: Value,
    },
    Length {
        at: Site,
        name: String,
        collection: Box<Tracked>,
    },
    Keys(Box<Tracked>),
    Choice(Vec<Tracked>),
    Unknown {
        effect: Value,
        held: BTreeSet<Token>,
    },
    Unbound,
}
impl Tracked {
    pub fn same_map_snapshot(&self, other: &Self) -> bool {
        if self != other {
            return false;
        }
        match self {
            Self::Input { parameter, .. } => parameter
                .path
                .iter()
                .all(|access| !matches!(access, Access::Element(_, None, _) | Access::Each(_))),
            Self::Map(entries) => entries.iter().all(|(key, _)| key.is_some()),
            Self::Iteration { .. } => true,
            _ => false,
        }
    }
    pub fn finish_iteration(&mut self, at: Site) {
        match self {
            Self::Keys(_) => *self = Self::data(Value::Collection(Box::new(Value::Plain))),
            Self::Length { .. } => *self = Self::data(Value::Plain),
            Self::Iteration {
                at: owner,
                base,
                path,
                effect,
            } if *owner == at => {
                *self = if path.is_empty() {
                    *base.clone()
                } else {
                    Self::unknown(effect.clone(), base.tokens())
                };
            }
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => {
                for value in fields.values_mut() {
                    value.finish_iteration(at);
                }
            }
            Self::List(values)
            | Self::Batch(values)
            | Self::Choice(values)
            | Self::Variant { fields: values, .. } => {
                for value in values {
                    value.finish_iteration(at);
                }
            }
            Self::Map(entries) => {
                for (_, value) in entries {
                    value.finish_iteration(at);
                }
            }
            Self::Future { result, .. } => {
                fn clear(result: &mut FutureResult, at: Site) {
                    match result {
                        FutureResult::Cases(cases) => {
                            for case in cases {
                                case.observed.retain(|token|!matches!(token,Token::Iteration(owner,_) if *owner==at));
                                clear(&mut case.result, at);
                            }
                        }
                        FutureResult::Pending(held) => held.retain(
                            |token| !matches!(token,Token::Iteration(owner,_) if *owner==at),
                        ),
                        _ => {}
                    }
                }
                clear(result, at);
                result.map_values(&mut |value| value.finish_iteration(at))
            }
            _ => {}
        }
    }
    pub fn prepare_return(
        &mut self,
        resolved: &BTreeMap<Token, BTreeSet<usize>>,
        pending: &BTreeMap<Token, Obligation>,
        population: &BTreeSet<Token>,
    ) {
        match self {
            Self::Members(members) => {
                members.population = members.members.intersection(population).cloned().collect();
                members.pending = members
                    .members
                    .iter()
                    .filter(|token| pending.contains_key(*token))
                    .cloned()
                    .collect()
            }
            Self::Keys(_) => *self = Self::data(Value::Collection(Box::new(Value::Plain))),
            Self::Length { .. } => *self = Self::data(Value::Plain),
            Self::Future { token, result, .. } => {
                if let FutureResult::Cases(cases) = result {
                    if let Some(selected) = resolved.get(token) {
                        let mut index = 0;
                        cases.retain(|_| {
                            let keep = selected.contains(&index);
                            index += 1;
                            keep
                        });
                    }
                    for case in cases {
                        case.created.retain(|token, _| pending.contains_key(token));
                    }
                }
                result.map_values(&mut |value| value.prepare_return(resolved, pending, population));
            }
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => {
                for value in fields.values_mut() {
                    value.prepare_return(resolved, pending, population);
                }
            }
            Self::List(values)
            | Self::Batch(values)
            | Self::Choice(values)
            | Self::Variant { fields: values, .. } => {
                for value in values {
                    value.prepare_return(resolved, pending, population);
                }
            }
            Self::Map(entries) => {
                for (_, value) in entries {
                    value.prepare_return(resolved, pending, population);
                }
            }
            _ => {}
        }
    }
    pub fn data(effect: Value) -> Self {
        Self::Data { effect, key: None }
    }
    pub fn unknown(effect: Value, held: BTreeSet<Token>) -> Self {
        Self::Unknown { effect, held }
    }
    pub fn effect(&self) -> Value {
        match self {
            Self::Reference { ty, .. } => Value::Opaque(ty.clone()),
            Self::Members(members) => Value::Collection(Box::new(members.element.clone())),
            Self::AllMembers { effect, .. } => effect.clone(),
            Self::Data { effect, .. }
            | Self::Input { effect, .. }
            | Self::Unknown { effect, .. } => effect.clone(),
            Self::Future { result, .. } => result.effect(),
            Self::Record { ty, fields } => Value::Aggregate {
                ty: ty.clone(),
                fields: fields
                    .iter()
                    .map(|(name, value)| (name.clone(), value.effect()))
                    .collect(),
            },
            Self::Variant { ty, name, fields } => Value::Variants {
                ty: ty.clone(),
                variants: BTreeMap::from([(
                    name.clone(),
                    fields.iter().map(Self::effect).collect(),
                )]),
            },
            Self::List(values) => Value::Collection(Box::new(join_effects(values.iter()))),
            Self::Map(entries) => Value::Collection(Box::new(join_effects(
                entries.iter().map(|(_, value)| value),
            ))),
            Self::Batch(values) => join_effects(values.iter()),
            Self::Iteration { effect, .. } => effect.clone(),
            Self::Length { .. } => Value::Plain,
            Self::Keys(_) => Value::Collection(Box::new(Value::Plain)),
            Self::Choice(values) => join_effects(values.iter()),
            Self::Unbound => Value::Bottom,
        }
    }
    pub fn key(&self) -> Option<Key> {
        match self {
            Self::Data {
                key: Some(Key::Iteration(_)),
                ..
            } => None,
            Self::Data { key, .. } => key.clone(),
            _ => None,
        }
    }
    pub fn tokens(&self) -> BTreeSet<Token> {
        match self {
            Self::Members(members) => members.members.clone(),
            Self::AllMembers {
                tokens, population, ..
            } => tokens.union(population).cloned().collect(),
            Self::Future { token, result, .. } => {
                let mut tokens = BTreeSet::from([token.clone()]);
                tokens.extend(result.tokens());
                tokens
            }
            Self::Input {
                parameter,
                future: true,
                ..
            } => BTreeSet::from([Token::Input(parameter.clone())]),
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => fields.values().flat_map(Self::tokens).collect(),
            Self::Variant { fields, .. } | Self::List(fields) | Self::Choice(fields) => {
                fields.iter().flat_map(Self::tokens).collect()
            }
            Self::Map(entries) => entries
                .iter()
                .flat_map(|(_, value)| value.tokens())
                .collect(),
            Self::Batch(values) => values.iter().flat_map(Self::tokens).collect(),
            Self::Iteration { base, .. } => base.tokens(),
            Self::Unknown { held, .. } => held.clone(),
            _ => BTreeSet::new(),
        }
    }
    pub fn guaranteed(&self) -> BTreeSet<Token> {
        match self {
            Self::Members(members) => members.complete.clone(),
            Self::Future { token, result, .. } => {
                let mut tokens = if token.observable() {
                    BTreeSet::from([token.clone()])
                } else {
                    BTreeSet::new()
                };
                tokens.extend(result.guaranteed());
                tokens
            }
            Self::Input {
                parameter,
                future: true,
                ..
            } => BTreeSet::from([Token::Input(parameter.clone())]),
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => fields.values().flat_map(Self::guaranteed).collect(),
            Self::Variant { fields, .. } | Self::List(fields) => {
                fields.iter().flat_map(Self::guaranteed).collect()
            }
            Self::Map(entries) => entries
                .iter()
                .enumerate()
                .filter(|(index, _)| map_entry_retained(entries, *index))
                .flat_map(|(_, (_, value))| value.guaranteed())
                .collect(),
            Self::Batch(values) => values.iter().flat_map(Self::guaranteed).collect(),
            Self::Choice(values) => {
                let mut sets = values.iter().map(Self::guaranteed);
                let mut common = sets.next().unwrap_or_default();
                for other in sets {
                    common.retain(|token| other.contains(token));
                }
                common
            }
            _ => BTreeSet::new(),
        }
    }
    pub fn project(&self, access: Access, fallback: Value, future: bool) -> Self {
        match (self, &access) {
            (Self::Members(members), Access::Each(_)) => Self::AllMembers {
                population: members.population_complete.clone(),
                tokens: members.complete.clone(),
                effect: members.element.clone(),
            },
            (Self::Members(members), Access::Element(..)) => {
                Self::unknown(members.element.clone(), members.members.clone())
            }
            (Self::Keys(map), Access::MapValues(_)) => *map.clone(),
            (Self::List(values), Access::Each(_)) => Self::Batch(values.clone()),
            (Self::Map(entries), Access::Each(_)) => Self::Batch(
                entries
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| map_entry_retained(entries, *index))
                    .map(|(_, (_, value))| value.clone())
                    .collect(),
            ),
            (Self::Batch(values), _) => Self::Batch(
                values
                    .iter()
                    .map(|value| value.project(access.clone(), fallback.clone(), future))
                    .collect(),
            ),
            (
                Self::Iteration {
                    at,
                    base,
                    path,
                    effect,
                },
                _,
            ) => {
                let mut path = path.clone();
                path.push(access.clone());
                Self::Iteration {
                    at: *at,
                    base: base.clone(),
                    path,
                    effect: effect.project(access.effect(), fallback),
                }
            }
            (Self::Record { fields, .. }, Access::Field(_, name)) => fields
                .get(name)
                .cloned()
                .unwrap_or_else(|| Self::unknown(fallback, self.tokens())),
            (Self::Variant { name, fields, .. }, Access::Variant(_, expected, index))
                if name == expected =>
            {
                fields
                    .get(*index)
                    .cloned()
                    .unwrap_or_else(|| Self::unknown(fallback, self.tokens()))
            }
            (Self::List(values), Access::Element(_, Some(Key::Int(index)), _)) => {
                usize::try_from(*index)
                    .ok()
                    .and_then(|i| values.get(i))
                    .cloned()
                    .unwrap_or_else(|| Self::unknown(fallback, BTreeSet::new()))
            }
            (Self::Map(entries), Access::Element(_, Some(key), _)) => choice(
                entries
                    .iter()
                    .filter(|(candidate, _)| {
                        candidate.as_ref().is_none_or(|candidate| candidate == key)
                    })
                    .map(|(_, value)| value.clone())
                    .collect(),
                fallback,
            ),
            (Self::List(values), Access::Element(_, None, _)) => choice(values.clone(), fallback),
            (Self::Map(entries), Access::Element(_, None, _)) => choice(
                entries.iter().map(|(_, value)| value.clone()).collect(),
                fallback,
            ),
            (
                Self::Future {
                    result: FutureResult::Success(value),
                    ..
                },
                Access::Await(_),
            ) => *value.clone(),
            (
                Self::Input {
                    parameter, effect, ..
                },
                _,
            ) => {
                if parameter.path.contains(&access) {
                    return Self::unknown(fallback, self.tokens());
                }
                let mut parameter = parameter.clone();
                parameter.path.push(access.clone());
                Self::Input {
                    parameter,
                    effect: effect.project(access.effect(), fallback),
                    future,
                }
            }
            (Self::Choice(values), _) => choice(
                values
                    .iter()
                    .map(|value| value.project(access.clone(), fallback.clone(), future))
                    .collect(),
                fallback,
            ),
            (Self::Unknown { effect, .. }, _) => {
                Self::unknown(effect.project(access.effect(), fallback), self.tokens())
            }
            _ => Self::unknown(fallback, self.tokens()),
        }
    }
    pub fn alternatives(&self) -> Vec<Self> {
        match self {
            Self::Choice(values) => values.iter().flat_map(Self::alternatives).collect(),
            value => vec![value.clone()],
        }
    }
    pub fn rename(&mut self, from: &Token, to: &Token) {
        fn rename(token: &mut Token, from: &Token, to: &Token) {
            match token {
                Token::All(inner) | Token::Population(inner) => rename(inner, from, to),
                token if token == from => *token = to.clone(),
                _ => {}
            }
        }
        self.visit_tokens(&mut |token, _| {
            rename(token, from, to);
        });
    }
    pub fn visit_tokens(&mut self, visit: &mut impl FnMut(&mut Token, &mut bool)) {
        match self {
            Self::Members(members) => members.visit_tokens(visit),
            Self::AllMembers {
                tokens, population, ..
            } => {
                visit_set(tokens, visit);
                visit_set(population, visit);
            }
            Self::Future {
                token,
                result,
                pending,
            } => {
                visit(token, pending);
                result.visit_tokens(visit);
            }
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => {
                for value in fields.values_mut() {
                    value.visit_tokens(visit);
                }
            }
            Self::List(values)
            | Self::Batch(values)
            | Self::Choice(values)
            | Self::Variant { fields: values, .. } => {
                for value in values {
                    value.visit_tokens(visit);
                }
            }
            Self::Map(entries) => {
                for (_, value) in entries {
                    value.visit_tokens(visit);
                }
            }
            Self::Unknown { held, .. } => {
                *held = std::mem::take(held)
                    .into_iter()
                    .map(|mut token| {
                        visit(&mut token, &mut false);
                        token
                    })
                    .collect()
            }
            Self::Iteration { base, .. } => base.visit_tokens(visit),
            _ => {}
        }
    }
    pub fn join(&mut self, other: Self) {
        if *self == other {
            return;
        }
        match (&mut *self, other) {
            (left @ Self::Future { .. }, right @ Self::Future { .. }) if matches!((&*left,&right),(Self::Future {token:first,..},Self::Future {token:second,..}) if first==second) =>
            {
                let first = left.clone();
                *left = choice(vec![first, right], Value::Plain);
            }
            (Self::Data { effect, key }, Self::Data { effect: rhs, .. }) => {
                effect.join(rhs);
                *key = None;
            }
            (Self::List(left), Self::List(right)) if left.len() == right.len() => {
                for (left, right) in left.iter_mut().zip(right) {
                    left.join(right);
                }
            }
            (
                Self::Record { ty, fields },
                Self::Record {
                    ty: rhs,
                    fields: other,
                },
            ) if *ty == rhs => {
                for (name, value) in fields {
                    if let Some(other) = other.get(name) {
                        value.join(other.clone());
                    }
                }
            }
            (left, right) => {
                let mut held = left.tokens();
                held.extend(right.tokens());
                let mut effect = left.effect();
                effect.join(right.effect());
                *left = Self::unknown(effect, held);
            }
        }
    }
    pub fn finite(self) -> Self {
        fn visit(value: Tracked, seen: &mut BTreeSet<String>) -> Tracked {
            match value {
                Tracked::Reference { ref ty, .. } if seen.contains(ty) => {
                    Tracked::unknown(value.effect(), value.tokens())
                }
                Tracked::Reference {
                    identity,
                    ty,
                    snapshot,
                } => {
                    seen.insert(ty.clone());
                    let snapshot = snapshot.map(|fields| {
                        fields
                            .into_iter()
                            .map(|(name, value)| (name, visit(value, seen)))
                            .collect()
                    });
                    seen.remove(&ty);
                    Tracked::Reference {
                        identity,
                        ty,
                        snapshot,
                    }
                }
                Tracked::Record { ref ty, .. } | Tracked::Variant { ref ty, .. }
                    if seen.contains(ty) =>
                {
                    Tracked::unknown(value.effect().finite(), value.tokens())
                }
                Tracked::Record { ty, fields } => {
                    seen.insert(ty.clone());
                    let fields = fields
                        .into_iter()
                        .map(|(name, value)| (name, visit(value, seen)))
                        .collect();
                    seen.remove(&ty);
                    Tracked::Record { ty, fields }
                }
                Tracked::Variant { ty, name, fields } => {
                    seen.insert(ty.clone());
                    let fields = fields.into_iter().map(|value| visit(value, seen)).collect();
                    seen.remove(&ty);
                    Tracked::Variant { ty, name, fields }
                }
                Tracked::Future {
                    token,
                    pending,
                    mut result,
                } => {
                    result.map_values(&mut |value| *value = visit(value.clone(), seen));
                    Tracked::Future {
                        token,
                        pending,
                        result,
                    }
                }
                Tracked::List(values) => {
                    Tracked::List(values.into_iter().map(|value| visit(value, seen)).collect())
                }
                Tracked::Map(entries) => Tracked::Map(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, visit(value, seen)))
                        .collect(),
                ),
                Tracked::Choice(values) => choice(
                    values.into_iter().map(|value| visit(value, seen)).collect(),
                    Value::Plain,
                ),
                value => value,
            }
        }
        visit(self, &mut BTreeSet::new())
    }
    pub fn erase_opaque_local_tokens(&mut self) {
        match self {
            Self::Unknown { held, .. } => held.retain(|token| matches!(token, Token::Input(_))),
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => {
                for value in fields.values_mut() {
                    value.erase_opaque_local_tokens();
                }
            }
            Self::List(values) | Self::Choice(values) | Self::Variant { fields: values, .. } => {
                for value in values {
                    value.erase_opaque_local_tokens();
                }
            }
            Self::Map(entries) => {
                for (_, value) in entries {
                    value.erase_opaque_local_tokens();
                }
            }
            Self::Future { result, .. } => result.map_values(&mut Self::erase_opaque_local_tokens),
            _ => {}
        }
    }
    pub fn widen_recursive_return(&mut self) {
        match self {
            Self::Members(_) => *self = Self::unknown(self.effect().finite(), self.tokens()),
            Self::Data {
                key: Some(Key::Int(_)),
                ..
            } => {
                if let Self::Data { key, .. } = self {
                    *key = None;
                }
            }
            Self::List(_) | Self::Map(_) => {
                *self = Self::unknown(self.effect().finite(), self.tokens())
            }
            Self::Record { fields, .. }
            | Self::Reference {
                snapshot: Some(fields),
                ..
            } => {
                for value in fields.values_mut() {
                    value.widen_recursive_return();
                }
            }
            Self::Choice(values) | Self::Variant { fields: values, .. } => {
                for value in values {
                    value.widen_recursive_return();
                }
            }
            Self::Future { result, .. } => result.map_values(&mut Self::widen_recursive_return),
            _ => {}
        }
    }
}
pub(super) fn choice(mut values: Vec<Tracked>, fallback: Value) -> Tracked {
    let mut distinct = Vec::new();
    for value in values.drain(..).flat_map(|value| value.alternatives()) {
        if !distinct.contains(&value) {
            distinct.push(value);
        }
    }
    match distinct.len() {
        0 => Tracked::unknown(fallback, BTreeSet::new()),
        1 => distinct.pop().unwrap(),
        _ => Tracked::Choice(distinct),
    }
}
fn map_entry_retained(entries: &[(Option<Key>, Tracked)], index: usize) -> bool {
    entries[index+1..].iter().all(|(later,_)|matches!((&entries[index].0,later),(Some(first),Some(second)) if first!=second))
}
pub(super) fn map(entries: impl IntoIterator<Item = (Option<Key>, Tracked)>) -> Tracked {
    let mut result: Vec<(Option<Key>, Tracked)> = Vec::new();
    for (key, value) in entries {
        if let Some(key) = &key {
            result.retain(|(previous, _)| previous.as_ref() != Some(key));
        }
        result.push((key, value));
    }
    Tracked::Map(result)
}
fn join_effects<'a>(values: impl Iterator<Item = &'a Tracked>) -> Value {
    let mut result = Value::Bottom;
    for value in values {
        result.join(value.effect());
    }
    result
}
