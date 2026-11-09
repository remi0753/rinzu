//! Substitute complete failure words, choosing each Future input's word once.
//! Independent position substitution would invent [A,A] from [A,B] | [B,A].
use super::*;

fn argument(input: &Input, arguments: &[Value], fallback: Value) -> Value {
    let mut actual = arguments
        .get(input.index)
        .cloned()
        .unwrap_or_else(|| fallback.clone());
    for projection in &input.path {
        actual = actual.project(projection.clone(), fallback.clone());
    }
    actual
}
fn unknown_future(bound: usize) -> Value {
    Value::Future {
        errors: language::unknown(bound),
        success: true,
        value: Box::new(Value::Plain),
    }
}
fn tag_value(tag: &Tag, name: &str, arguments: &[Value], bound: usize) -> Value {
    match tag {
        Tag::Parameter(input) if input.owner == name => {
            argument(input, arguments, Value::Error(BTreeSet::from([Tag::Any])))
        }
        Tag::FutureElement(input, index) if input.owner == name => {
            let mut result = Value::Bottom;
            for word in argument(input, arguments, unknown_future(bound)).future_errors(bound) {
                if let Some(tag) = word.prefix.get(*index) {
                    result
                        .join(Value::Error(BTreeSet::from([tag.clone()])).conditioned(word.guards));
                } else if *index >= bound && word.longer {
                    result.join(Value::Error(word.tail).conditioned(word.guards));
                }
            }
            result
        }
        tag => Value::Error(BTreeSet::from([tag.clone()])),
    }
}
fn alternatives(
    conditions: &language::Guards,
    name: &str,
    arguments: &[Value],
    bound: usize,
) -> Vec<(language::Guards, BTreeMap<Input, language::Word>)> {
    let preserved = conditions
        .iter()
        .filter(|(input, _)| input.owner != name)
        .map(|(input, guard)| (input.clone(), guard.clone()))
        .collect();
    let mut alternatives = vec![(preserved, BTreeMap::<Input, language::Word>::new())];
    for (input, guard) in conditions.iter().filter(|(input, _)| input.owner == name) {
        let mut expanded = Vec::new();
        match guard {
            language::Guard::FutureNeverReturns | language::Guard::FuturePanics => {
                let actual = argument(input, arguments, Value::Plain);
                let paths = if matches!(guard, language::Guard::FuturePanics) {
                    actual.panic_guards()
                } else {
                    actual.nonreturn_guards()
                };
                for (guards, selected) in &alternatives {
                    for candidate in &paths {
                        if let Some(guards) = language::merge_guards(guards, candidate) {
                            expanded.push((guards, selected.clone()));
                        }
                    }
                }
            }
            language::Guard::FutureSuccess | language::Guard::Bool(_) => {
                let actual = argument(input, arguments, Value::Plain);
                let paths = if let language::Guard::Bool(expected) = guard {
                    actual.bool_guards(*expected)
                } else {
                    actual.success_guards()
                };
                for (guards, selected) in &alternatives {
                    for candidate in &paths {
                        if let Some(guards) = language::merge_guards(guards, candidate) {
                            expanded.push((guards, selected.clone()));
                        }
                    }
                }
            }
            language::Guard::Future(shape) => {
                let actual = argument(input, arguments, unknown_future(bound)).future_errors(bound);
                for (guards, selected) in &alternatives {
                    for candidate in &actual {
                        if shape.is_some_and(|shape| {
                            shape.length != candidate.prefix.len()
                                || shape.longer != candidate.longer
                        }) {
                            continue;
                        }
                        if let Some(guards) = language::merge_guards(guards, &candidate.guards) {
                            let mut selected = selected.clone();
                            selected.insert(input.clone(), candidate.clone());
                            expanded.push((guards, selected));
                        }
                    }
                }
            }
            language::Guard::CaughtType(expected) => {
                let mut base = input.clone();
                let position = if let Some(Projection::FailureElement(index)) = base.path.last() {
                    let index = *index;
                    base.path.pop();
                    Some(index)
                } else {
                    None
                };
                let expected = Tag::Named(expected.clone());
                for (guards, selected) in &alternatives {
                    let candidates = if position.is_none() {
                        argument(input, arguments, Value::Error(BTreeSet::from([Tag::Any])))
                            .thrown_errors()
                    } else if let Some(candidate) = selected.get(&base) {
                        Language::from([candidate.clone()])
                    } else {
                        argument(&base, arguments, unknown_future(bound)).future_errors(bound)
                    };
                    for mut candidate in candidates {
                        let index = position.unwrap_or(0);
                        if candidate.prefix.get(index).is_some()
                            && language::narrow_tag(&mut candidate, index, &expected)
                            && let Some(guards) = language::merge_guards(guards, &candidate.guards)
                        {
                            let mut selected = selected.clone();
                            if position.is_some() {
                                selected.insert(base.clone(), candidate);
                            }
                            expanded.push((guards, selected));
                        }
                    }
                }
            }
            language::Guard::Variant(variant) => {
                let actual = argument(input, arguments, Value::Plain).selection_guards(variant);
                for (guards, selected) in &alternatives {
                    for candidate in &actual {
                        if let Some(guards) = language::merge_guards(guards, candidate) {
                            expanded.push((guards, selected.clone()));
                        }
                    }
                }
            }
            language::Guard::Target(target) => {
                let actual = argument(input, arguments, Value::Plain).target_guards(target);
                for (guards, selected) in &alternatives {
                    for candidate in &actual {
                        if let Some(guards) = language::merge_guards(guards, candidate) {
                            expanded.push((guards, selected.clone()));
                        }
                    }
                }
            }
        }
        alternatives = expanded;
    }
    alternatives
}
pub(super) fn language(
    errors: &Language,
    name: &str,
    arguments: &[Value],
    bound: usize,
) -> Language {
    let mut result = Language::new();
    for word in errors {
        for (guards, selected) in alternatives(&word.guards, name, arguments, bound) {
            let mut prefixes = vec![(Vec::new(), guards)];
            for tag in &word.prefix {
                let replacements = if let Tag::FutureElement(input, index) = tag
                    && input.owner == name
                {
                    if language::element_input(input) {
                        tag_value(tag, name, arguments, bound).thrown_errors()
                    } else {
                        selected
                            .get(input)
                            .and_then(|word| word.prefix.get(*index))
                            .cloned()
                            .into_iter()
                            .flat_map(language::one)
                            .collect()
                    }
                } else {
                    tag_value(tag, name, arguments, bound).thrown_errors()
                };
                let mut expanded = Vec::new();
                for (prefix, guards) in prefixes {
                    for replacement in &replacements {
                        if let Some(guards) = language::merge_guards(&guards, &replacement.guards) {
                            let mut prefix = prefix.clone();
                            prefix.extend(replacement.prefix.iter().cloned());
                            expanded.push((prefix, guards));
                        }
                    }
                }
                prefixes = expanded;
            }
            for (prefix, guards) in prefixes {
                let mut tail = BTreeSet::new();
                for tag in &word.tail {
                    if let Tag::FutureElement(input, index) = tag
                        && input.owner == name
                        && !language::element_input(input)
                    {
                        if let Some(actual) = selected.get(input) {
                            if let Some(tag) = actual.prefix.get(*index) {
                                tail.insert(tag.clone());
                            } else {
                                tail.extend(actual.tail.iter().cloned());
                            }
                        }
                    } else {
                        for replacement in tag_value(tag, name, arguments, bound).thrown_errors() {
                            if language::merge_guards(&guards, &replacement.guards).is_some() {
                                tail.extend(replacement.prefix);
                                tail.extend(replacement.tail);
                            }
                        }
                    }
                }
                result.insert(language::Word {
                    prefix,
                    tail,
                    longer: word.longer,
                    guards,
                });
            }
        }
    }
    language::normalize(result)
}
pub(super) fn value(input: &Value, name: &str, arguments: &[Value], bound: usize) -> Value {
    match input {
        Value::Parameter {
            owner,
            index,
            path,
            fallback,
        } if owner == name => argument(
            &Input {
                owner: owner.clone(),
                index: *index,
                path: path.clone(),
            },
            arguments,
            *fallback.clone(),
        ),
        Value::Aggregate { ty, fields } => Value::Aggregate {
            ty: ty.clone(),
            fields: fields
                .iter()
                .map(|(field, child)| (field.clone(), value(child, name, arguments, bound)))
                .collect(),
        },
        Value::Variants { ty, variants } => Value::Variants {
            ty: ty.clone(),
            variants: variants
                .iter()
                .map(|(variant, fields)| {
                    (
                        variant.clone(),
                        fields
                            .iter()
                            .map(|child| value(child, name, arguments, bound))
                            .collect(),
                    )
                })
                .collect(),
        },
        Value::Collection(child) => {
            Value::Collection(Box::new(value(child, name, arguments, bound)))
        }
        Value::Union(alternatives) => {
            let mut result = Value::Bottom;
            for alternative in alternatives {
                result.join(value(alternative, name, arguments, bound));
            }
            result
        }
        Value::Unbound(child) => Value::Unbound(Box::new(value(child, name, arguments, bound))),
        Value::Error(errors) => {
            let mut result = Value::Bottom;
            for tag in errors {
                result.join(tag_value(tag, name, arguments, bound));
            }
            result
        }
        Value::Conditional {
            guards,
            value: child,
        } => {
            let mut result = Value::Bottom;
            for (guards, _) in alternatives(guards, name, arguments, bound) {
                result.join(value(child, name, arguments, bound).conditioned(guards));
            }
            result
        }
        Value::Future {
            errors,
            success,
            value: returned,
        } => Value::Future {
            errors: language(errors, name, arguments, bound),
            success: *success,
            value: Box::new(value(returned, name, arguments, bound)),
        },
        value => value.clone(),
    }
}
pub(super) fn instantiate(
    summary: &Summary,
    name: &str,
    arguments: &[Value],
    bound: usize,
) -> Summary {
    Summary {
        errors: language(&summary.errors, name, arguments, bound),
        success: summary.success,
        returned: value(&summary.returned, name, arguments, bound),
    }
}
