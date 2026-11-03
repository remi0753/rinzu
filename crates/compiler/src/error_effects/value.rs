//! Abstract values preserve latent Future failures through immutable values.
//! Unknown or mutable storage is never treated as an infallible Future.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum Projection {
    Field(String, String),
    Element(String, (u32, u32, u32, u32, u32)),
    AllElements(String),
    AnyFailure(String),
    MapValues(String),
    FailureShape(usize, bool),
    Variant(String, String, usize),
    Await(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_partition_and_merge_has_a_stable_abstract_value() {
        let input = Input {
            owner: "choose".into(),
            index: 0,
            path: vec![],
        };
        let yes = language::Guards::from([(input.clone(), language::Guard::Bool(true))]);
        let no = language::Guards::from([(input, language::Guard::Bool(false))]);
        let mut original =
            Value::Callable(BTreeSet::from(["first".into()])).conditioned(yes.clone());
        original.join(Value::Callable(BTreeSet::from(["second".into()])).conditioned(no.clone()));
        let mut value = original.clone();
        for _ in 0..128 {
            let mut partition = value.clone().conditioned(yes.clone());
            partition.join(value.conditioned(no.clone()));
            assert_eq!(partition, original);
            value = partition;
        }
        assert_eq!(value.target_guards("first"), vec![yes]);
        assert_eq!(value.target_guards("second"), vec![no]);
    }

    #[test]
    fn normalizing_distributed_conditions_flattens_nested_alternatives() {
        let input = Input {
            owner: "route".into(),
            index: 0,
            path: vec![],
        };
        let guards = language::Guards::from([(input, language::Guard::Bool(true))]);
        let first = Value::Callable(BTreeSet::from(["first".into()]));
        let second = Value::Callable(BTreeSet::from(["second".into()]));
        let third = Value::Callable(BTreeSet::from(["third".into()]));
        // Normalizing the child distributes its condition and creates a union.
        // The parent must flatten that union in the same normalization pass.
        let value = Value::Union(BTreeSet::from([
            Value::Conditional {
                guards: guards.clone(),
                value: Box::new(Value::Union(BTreeSet::from([
                    first.clone(),
                    second.clone(),
                ]))),
            },
            third.clone(),
        ]));
        let expected = Value::Union(BTreeSet::from([
            first.conditioned(guards.clone()),
            second.conditioned(guards.clone()),
            third,
        ]));
        assert_eq!(value.finite(), expected);
        assert_eq!(expected.target_guards("first"), vec![guards]);
        assert_eq!(
            expected.target_guards("third"),
            vec![language::Guards::new()]
        );
        assert!(expected.target_guards("absent").is_empty());
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) enum Value {
    #[default]
    Bottom,
    Plain,
    Opaque(String),
    Callable(BTreeSet<String>),
    Union(BTreeSet<Value>),
    Conditional {
        guards: language::Guards,
        value: Box<Value>,
    },
    Bool(Option<bool>),
    Error(BTreeSet<Tag>),
    Unbound(Box<Value>),
    Future {
        errors: Language,
        success: bool,
        value: Box<Value>,
    },
    /// Proven to have no normal completion; unlike an I/O wait this is not
    /// merely a Future whose result is not ready yet.
    NeverFuture,
    /// Completes by unwinding with an uncatchable panic; finally still runs.
    PanicFuture,
    Parameter {
        owner: String,
        index: usize,
        path: Vec<Projection>,
        fallback: Box<Value>,
    },
    Aggregate {
        ty: String,
        fields: BTreeMap<String, Value>,
    },
    Variants {
        ty: String,
        variants: BTreeMap<String, Vec<Value>>,
    },
    Collection(Box<Value>),
}

impl Value {
    pub(super) fn panic_guards(&self) -> Vec<language::Guards> {
        match self {
            Self::PanicFuture => vec![BTreeMap::new()],
            Self::Parameter {
                owner, index, path, ..
            } => vec![BTreeMap::from([(
                Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                },
                language::Guard::FuturePanics,
            )])],
            Self::Union(values) => values.iter().flat_map(Self::panic_guards).collect(),
            Self::Conditional { guards, value } => value
                .panic_guards()
                .iter()
                .filter_map(|candidate| language::merge_guards(guards, candidate))
                .collect(),
            _ => Vec::new(),
        }
    }
    pub(super) fn nonreturn_guards(&self) -> Vec<language::Guards> {
        match self {
            Self::NeverFuture => vec![BTreeMap::new()],
            Self::Future {
                errors, success, ..
            } if !*success && errors.is_empty() => vec![BTreeMap::new()],
            Self::Parameter {
                owner, index, path, ..
            } => {
                vec![BTreeMap::from([(
                    Input {
                        owner: owner.clone(),
                        index: *index,
                        path: path.clone(),
                    },
                    language::Guard::FutureNeverReturns,
                )])]
            }
            Self::Union(values) => values.iter().flat_map(Self::nonreturn_guards).collect(),
            Self::Conditional { guards, value } => value
                .nonreturn_guards()
                .iter()
                .filter_map(|candidate| language::merge_guards(guards, candidate))
                .collect(),
            _ => Vec::new(),
        }
    }
    /// A persistent collection operation holds an abstract member set. A read
    /// selects one value from that set, separately from another read site.
    fn select_elements(&self, at: (u32, u32, u32, u32, u32)) -> Self {
        match self {
            Self::Parameter {
                owner,
                index,
                path,
                fallback,
            } => Self::Parameter {
                owner: owner.clone(),
                index: *index,
                path: path
                    .iter()
                    .map(|projection| {
                        if let Projection::AllElements(ty) = projection {
                            Projection::Element(ty.clone(), at)
                        } else {
                            projection.clone()
                        }
                    })
                    .collect(),
                fallback: fallback.clone(),
            },
            Self::Union(values) => {
                let mut value = Self::Bottom;
                for member in values {
                    value.join(member.select_elements(at));
                }
                value
            }
            Self::Conditional { guards, value } => {
                value.select_elements(at).conditioned(guards.clone())
            }
            value => value.clone(),
        }
    }
    /// Loop-carried values can come from a different element on the next
    /// iteration. Preserve stable inputs, and widen element-specific facts.
    pub(super) fn forget_elements(&self) -> Self {
        match self {
            Self::Parameter {
                owner,
                index,
                path,
                fallback,
            } if language::element_input(&Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            }) =>
            {
                fallback.forget_elements()
            }
            Self::Future {
                errors,
                success,
                value,
            } => Self::Future {
                errors: language::independent_elements(errors),
                success: *success,
                value: Box::new(value.forget_elements()),
            },
            Self::Conditional { guards, value } => {
                if guards.keys().any(language::element_input) {
                    value.forget_elements()
                } else {
                    value.forget_elements().conditioned(guards.clone())
                }
            }
            Self::Error(tags) => Self::Error(
                tags.iter()
                    .map(|tag| match tag {
                        Tag::Parameter(input) | Tag::FutureElement(input, _)
                            if language::element_input(input) =>
                        {
                            Tag::Any
                        }
                        tag => tag.clone(),
                    })
                    .collect(),
            ),
            Self::Aggregate { ty, fields } => Self::Aggregate {
                ty: ty.clone(),
                fields: fields
                    .iter()
                    .map(|(name, value)| (name.clone(), value.forget_elements()))
                    .collect(),
            },
            Self::Variants { ty, variants } => Self::Variants {
                ty: ty.clone(),
                variants: variants
                    .iter()
                    .map(|(name, fields)| {
                        (
                            name.clone(),
                            fields.iter().map(Self::forget_elements).collect(),
                        )
                    })
                    .collect(),
            },
            Self::Collection(value) => Self::Collection(Box::new(value.forget_elements())),
            Self::Unbound(value) => Self::Unbound(Box::new(value.forget_elements())),
            Self::Union(values) => {
                let mut value = Self::Bottom;
                for member in values {
                    value.join(member.forget_elements());
                }
                value
            }
            value => value.clone(),
        }
    }
    pub(super) fn join(&mut self, other: Self) {
        if *self == other {
            return;
        }
        match (&mut *self, other) {
            (this @ Self::Bottom, other) => *this = other,
            (_, Self::Bottom) => {}
            (Self::Unbound(left), Self::Unbound(right)) => left.join(*right),
            (Self::Unbound(left), right) => left.join(right),
            (left, Self::Unbound(right)) => {
                let mut joined = std::mem::take(left);
                joined.join(*right);
                *left = Self::Unbound(Box::new(joined));
            }
            (Self::Error(left), Self::Error(right)) => left.extend(right),
            (Self::Callable(left), Self::Callable(right)) => left.extend(right),
            (
                Self::Future {
                    errors,
                    success,
                    value,
                },
                Self::Future {
                    errors: rhs,
                    success: ok,
                    value: ret,
                },
            ) => {
                errors.extend(rhs);
                *success |= ok;
                value.join(*ret);
            }
            (Self::Bool(left), Self::Bool(right)) => {
                if *left != right {
                    *left = None;
                }
            }
            (Self::Collection(left), Self::Collection(right)) => left.join(*right),
            (
                Self::Conditional { guards, value },
                Self::Conditional {
                    guards: other,
                    value: rhs,
                },
            ) if *guards == other => value.join(*rhs),
            (
                Self::Aggregate { ty, fields },
                Self::Aggregate {
                    ty: rhs,
                    fields: other,
                },
            ) if *ty == rhs => {
                fields.retain(|name, value| {
                    if let Some(other) = other.get(name) {
                        value.join(other.clone());
                        true
                    } else {
                        false
                    }
                });
            }
            (
                Self::Variants { ty, variants },
                Self::Variants {
                    ty: rhs,
                    variants: other,
                },
            ) if *ty == rhs => {
                for (name, values) in other {
                    if let Some(previous) = variants.get_mut(&name) {
                        for (previous, value) in previous.iter_mut().zip(values) {
                            previous.join(value);
                        }
                    } else {
                        variants.insert(name, values);
                    }
                }
            }
            (Self::Plain, _) => {}
            (left, Self::Plain) => *left = Self::Plain,
            (Self::Union(left), Self::Union(right)) => left.extend(right),
            (Self::Union(left), right) => {
                left.insert(right);
            }
            (left, right) => {
                let previous = std::mem::take(left);
                let mut alternatives = if let Self::Union(values) = right {
                    values
                } else {
                    BTreeSet::from([right])
                };
                alternatives.insert(previous);
                *left = Self::Union(alternatives);
            }
        }
        *self = std::mem::take(self).finite();
    }

    pub(super) fn project(&self, projection: Projection, fallback: Value) -> Value {
        match self {
            Self::Bottom => Self::Bottom,
            Self::Parameter {
                owner, index, path, ..
            } => {
                // A repeated typed projection requires a recursive type.
                // Keep its continuation unknown, without an arbitrary depth
                // limit or an unbounded path in recursive summaries.
                if path.contains(&projection) {
                    return fallback;
                }
                let mut path = path.clone();
                path.push(projection);
                Self::Parameter {
                    owner: owner.clone(),
                    index: *index,
                    path,
                    fallback: Box::new(fallback),
                }
            }
            Self::Aggregate { fields, .. } => {
                if let Projection::Field(_, name) = projection {
                    fields.get(&name).cloned().unwrap_or(fallback)
                } else {
                    fallback
                }
            }
            Self::Variants { variants, .. } => {
                if let Projection::Variant(_, name, index) = projection {
                    variants
                        .get(&name)
                        .and_then(|fields| fields.get(index))
                        .cloned()
                        .unwrap_or(fallback)
                } else {
                    fallback
                }
            }
            Self::Collection(element)
                if matches!(
                    projection,
                    Projection::Element(..)
                        | Projection::AllElements(_)
                        | Projection::AnyFailure(_)
                ) =>
            {
                if let Projection::Element(_, at) = projection {
                    element.select_elements(at)
                } else {
                    *element.clone()
                }
            }
            Self::Future { success, value, .. } if matches!(projection, Projection::Await(_)) => {
                if *success {
                    *value.clone()
                } else {
                    Self::Bottom
                }
            }
            Self::Future {
                errors,
                success,
                value,
            } if matches!(projection, Projection::FailureShape(..)) => {
                let Projection::FailureShape(length, longer) = projection else {
                    unreachable!()
                };
                Self::Future {
                    errors: errors
                        .iter()
                        .filter(|word| word.prefix.len() == length && word.longer == longer)
                        .cloned()
                        .collect(),
                    success: *success,
                    value: value.clone(),
                }
            }
            Self::Union(alternatives) => {
                let mut result = Self::Bottom;
                for value in alternatives {
                    result.join(value.project(projection.clone(), fallback.clone()));
                }
                result
            }
            Self::Conditional { guards, value } => value
                .project(projection, fallback)
                .conditioned(guards.clone()),
            _ => fallback,
        }
    }

    pub(super) fn nominal(&self) -> Option<&str> {
        match self {
            Self::Error(tags) if tags.len() == 1 => match tags.first() {
                Some(Tag::Named(name)) => Some(name),
                _ => None,
            },
            Self::Opaque(ty) | Self::Aggregate { ty, .. } | Self::Variants { ty, .. } => Some(ty),
            Self::Parameter { fallback, .. } | Self::Unbound(fallback) => fallback.nominal(),
            Self::Conditional { value, .. } => value.nominal(),
            _ => None,
        }
    }

    pub(super) fn error_tags(&self) -> BTreeSet<Tag> {
        match self {
            Self::Error(tags) => tags.clone(),
            Self::Parameter { fallback, .. } if matches!(fallback.as_ref(),Self::Error(tags) if !tags.is_empty() && tags.iter().all(|tag| matches!(tag,Tag::Named(_)))) =>
            {
                // A nominal Error field cannot contain a different Error
                // type, even when its enclosing closure is still symbolic.
                // Keep common Error inputs symbolic for caller substitution.
                fallback.error_tags()
            }
            Self::Parameter {
                owner, index, path, ..
            } => BTreeSet::from([Tag::Parameter(Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            })]),
            Self::Union(values) => values.iter().flat_map(Self::error_tags).collect(),
            Self::Conditional { value, .. } => value.error_tags(),
            Self::Bottom => BTreeSet::new(),
            _ => BTreeSet::from([Tag::Any]),
        }
    }
    pub(super) fn future_errors(&self, bound: usize) -> Language {
        match self {
            Self::NeverFuture | Self::PanicFuture => Language::new(),
            Self::Future { errors, .. } => errors.clone(),
            Self::Parameter {
                owner, index, path, ..
            } => language::future(
                Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                },
                bound,
            ),
            Self::Union(values) => values
                .iter()
                .flat_map(|value| value.future_errors(bound))
                .collect(),
            Self::Conditional { guards, value } => {
                language::restricted(&value.future_errors(bound), guards)
            }
            Self::Bottom => Language::new(),
            _ => language::unknown(bound),
        }
    }
    pub(super) fn awaited(&self, bound: usize, result: Value, ty: &str) -> (Language, bool, Value) {
        let failed = self.future_errors(bound);
        let (success, value) = match self {
            Self::NeverFuture | Self::PanicFuture => (false, Self::Bottom),
            Self::Future { success, value, .. } => (
                *success,
                if *success {
                    *value.clone()
                } else {
                    Self::Bottom
                },
            ),
            Self::Parameter { .. } => {
                (true, self.project(Projection::Await(ty.to_owned()), result))
            }
            Self::Union(values) => {
                let mut success = false;
                let mut returned = Self::Bottom;
                for value in values {
                    let (_, ok, value) = value.awaited(bound, result.clone(), ty);
                    success |= ok;
                    returned.join(value);
                }
                (success, returned)
            }
            Self::Conditional { guards, value } => {
                let (_, success, value) = value.awaited(bound, result, ty);
                (success, value.conditioned(guards.clone()))
            }
            Self::Bottom => (false, Self::Bottom),
            _ => (true, result),
        };
        (failed, success, value)
    }
    pub(super) fn conditioned(self, guards: language::Guards) -> Self {
        if guards.is_empty() || self == Self::Bottom {
            return self;
        }
        match self {
            Self::Conditional {
                guards: previous,
                value,
            } => language::merge_guards(&previous, &guards)
                .map_or(Self::Bottom, |guards| Self::Conditional { guards, value }),
            // Distribute restriction over alternatives. Wrapping the entire
            // union keeps copies of earlier unions at each branch merge and
            // grows exponentially even when the same conditions are repeated.
            Self::Union(values) => {
                // Do not re-enter join/finite here: finite is also a caller
                // and owns the visited nominal types for recursive values.
                let mut alternatives = BTreeSet::new();
                for value in values {
                    match value.conditioned(guards.clone()) {
                        Self::Bottom => {}
                        Self::Union(values) => alternatives.extend(values),
                        value => {
                            alternatives.insert(value);
                        }
                    }
                }
                match alternatives.len() {
                    0 => Self::Bottom,
                    1 => alternatives.pop_first().unwrap(),
                    _ => Self::Union(alternatives),
                }
            }
            value => Self::Conditional {
                guards,
                value: Box::new(value),
            },
        }
    }
    pub(super) fn selection_guards(&self, variant: &str) -> Vec<language::Guards> {
        match self {
            Self::Parameter {
                owner,
                index,
                path,
                fallback,
            } if language::element_input(&Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            }) =>
            {
                fallback.selection_guards(variant)
            }
            Self::Parameter {
                owner, index, path, ..
            } => vec![BTreeMap::from([(
                Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                },
                language::Guard::Variant(variant.to_owned()),
            )])],
            Self::Variants { variants, .. } => {
                if variants.contains_key(variant) {
                    vec![BTreeMap::new()]
                } else {
                    vec![]
                }
            }
            Self::Bottom => vec![],
            Self::Union(values) => values
                .iter()
                .flat_map(|value| value.selection_guards(variant))
                .collect(),
            Self::Conditional { guards, value } => value
                .selection_guards(variant)
                .iter()
                .filter_map(|path| language::merge_guards(guards, path))
                .collect(),
            _ => vec![BTreeMap::new()],
        }
    }
    pub(super) fn success_guards(&self) -> Vec<language::Guards> {
        match self {
            Self::NeverFuture | Self::PanicFuture => vec![],
            Self::Parameter {
                owner, index, path, ..
            } if language::element_input(&Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            }) =>
            {
                vec![BTreeMap::new()]
            }
            Self::Parameter {
                owner, index, path, ..
            } => vec![BTreeMap::from([(
                Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                },
                language::Guard::FutureSuccess,
            )])],
            Self::Future { success, .. } => {
                if *success {
                    vec![BTreeMap::new()]
                } else {
                    vec![]
                }
            }
            Self::Bottom => vec![],
            Self::Union(values) => values.iter().flat_map(Self::success_guards).collect(),
            Self::Conditional { guards, value } => value
                .success_guards()
                .iter()
                .filter_map(|path| language::merge_guards(guards, path))
                .collect(),
            _ => vec![BTreeMap::new()],
        }
    }
    pub(super) fn bool_guards(&self, expected: bool) -> Vec<language::Guards> {
        match self {
            Self::Parameter {
                owner,
                index,
                path,
                fallback,
            } if language::element_input(&Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            }) =>
            {
                fallback.bool_guards(expected)
            }
            Self::Parameter {
                owner, index, path, ..
            } => vec![BTreeMap::from([(
                Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                },
                language::Guard::Bool(expected),
            )])],
            Self::Bool(Some(value)) if *value != expected => vec![],
            Self::Bottom => vec![],
            Self::Union(values) => values
                .iter()
                .flat_map(|value| value.bool_guards(expected))
                .collect(),
            Self::Conditional { guards, value } => value
                .bool_guards(expected)
                .iter()
                .filter_map(|path| language::merge_guards(guards, path))
                .collect(),
            _ => vec![BTreeMap::new()],
        }
    }
    pub(super) fn target_guards(&self, target: &str) -> Vec<language::Guards> {
        match self {
            Self::Parameter {
                owner,
                index,
                path,
                fallback,
            } if language::element_input(&Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            }) =>
            {
                fallback.target_guards(target)
            }
            Self::Parameter {
                owner, index, path, ..
            } => vec![BTreeMap::from([(
                Input {
                    owner: owner.clone(),
                    index: *index,
                    path: path.clone(),
                },
                language::Guard::Target(target.to_owned()),
            )])],
            Self::Callable(targets) => {
                if targets.contains(target) {
                    vec![BTreeMap::new()]
                } else {
                    vec![]
                }
            }
            Self::Bottom => vec![],
            Self::Union(values) => values
                .iter()
                .flat_map(|value| value.target_guards(target))
                .collect(),
            Self::Conditional { guards, value } => value
                .target_guards(target)
                .iter()
                .filter_map(|path| language::merge_guards(guards, path))
                .collect(),
            _ => vec![BTreeMap::new()],
        }
    }
    pub(super) fn thrown_errors(&self) -> Language {
        match self {
            Self::Conditional { guards, value } => {
                language::restricted(&value.thrown_errors(), guards)
            }
            Self::Union(values) => values.iter().flat_map(Self::thrown_errors).collect(),
            value => value
                .error_tags()
                .into_iter()
                .flat_map(language::one)
                .collect(),
        }
    }

    /// Stop at a repeated nominal type, rather than allowing recursive source
    /// functions to grow an infinite abstract value. The unknown continuation
    /// remains conservative when a later expression projects a Future from it.
    pub(super) fn finite(self) -> Self {
        fn visit(value: Value, seen: &mut BTreeSet<String>) -> Value {
            match value {
                Value::Aggregate { ty, fields } => {
                    if !seen.insert(ty.clone()) {
                        return Value::Plain;
                    }
                    let fields = fields
                        .into_iter()
                        .map(|(name, value)| (name, visit(value, seen)))
                        .collect();
                    seen.remove(&ty);
                    Value::Aggregate { ty, fields }
                }
                Value::Variants { ty, variants } => {
                    if !seen.insert(ty.clone()) {
                        return Value::Plain;
                    }
                    let variants = variants
                        .into_iter()
                        .map(|(name, fields)| {
                            (
                                name,
                                fields.into_iter().map(|value| visit(value, seen)).collect(),
                            )
                        })
                        .collect();
                    seen.remove(&ty);
                    Value::Variants { ty, variants }
                }
                Value::Collection(value) => Value::Collection(Box::new(visit(*value, seen))),
                Value::Future {
                    errors,
                    success,
                    value,
                } => Value::Future {
                    errors,
                    success,
                    value: Box::new(visit(*value, seen)),
                },
                Value::Unbound(value) => Value::Unbound(Box::new(visit(*value, seen))),
                Value::Conditional { guards, value } => visit(*value, seen).conditioned(guards),
                Value::Union(values) => {
                    let mut alternatives = BTreeSet::new();
                    for value in values {
                        // A child condition may distribute into a new union.
                        // Keep alternatives flat so loop joins compare the
                        // same facts instead of growing nested copies.
                        match visit(value, seen) {
                            Value::Plain => return Value::Plain,
                            Value::Bottom => {}
                            Value::Union(values) => alternatives.extend(values),
                            value => {
                                alternatives.insert(value);
                            }
                        }
                    }
                    match alternatives.len() {
                        0 => Value::Bottom,
                        1 => alternatives.pop_first().unwrap(),
                        _ => Value::Union(alternatives),
                    }
                }
                value => value,
            }
        }
        visit(self, &mut BTreeSet::new())
    }
}
