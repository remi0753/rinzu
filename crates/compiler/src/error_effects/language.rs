//! Finite observations of error words. Catch patterns inspect only a bounded
//! prefix and exact length, so retaining that prefix plus a longer-word bit is
//! sufficient even for recursive propagation and repeated cleanup failures.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Input {
    pub owner: String,
    pub index: usize,
    pub path: Vec<super::Projection>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) struct Shape {
    pub length: usize,
    pub longer: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(super) enum Guard {
    Future(Option<Shape>),
    FutureSuccess,
    FutureNeverReturns,
    FuturePanics,
    Bool(bool),
    Variant(String),
    Target(String),
}
pub(super) type Guards = BTreeMap<Input, Guard>;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum Tag {
    Any,
    Named(String),
    Parameter(Input),
    FutureElement(Input, usize),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Word {
    pub prefix: Vec<Tag>,
    pub longer: bool,
    pub(super) guards: Guards,
}

pub(crate) type Language = BTreeSet<Word>;

pub(super) fn element_input(input: &Input) -> bool {
    input.path.iter().any(|projection| {
        matches!(
            projection,
            super::Projection::Element(..) | super::Projection::AllElements(_)
        )
    })
}
/// A new iteration can select a different element. Keep a separate collective
/// failure capability, rather than correlating its outcome with one prior read.
pub(super) fn independent_elements(errors: &Language) -> Language {
    errors
        .iter()
        .map(|word| {
            let prefix = word
                .prefix
                .iter()
                .map(|tag| match tag {
                    Tag::FutureElement(input, _) | Tag::Parameter(input)
                        if element_input(input) =>
                    {
                        Tag::Any
                    }
                    tag => tag.clone(),
                })
                .collect();
            let guards = word
                .guards
                .iter()
                .filter_map(|(input, guard)| {
                    if !element_input(input) {
                        return Some((input.clone(), guard.clone()));
                    }
                    if !matches!(guard, Guard::Future(_)) {
                        return None;
                    }
                    let mut input = input.clone();
                    for projection in &mut input.path {
                        if let super::Projection::Element(ty, _)
                        | super::Projection::AllElements(ty) = projection
                        {
                            *projection = super::Projection::AnyFailure(ty.clone());
                        }
                    }
                    Some((input, Guard::Future(None)))
                })
                .collect();
            Word {
                prefix,
                longer: word.longer,
                guards,
            }
        })
        .collect()
}

pub(crate) fn empty_word() -> Language {
    BTreeSet::from([Word {
        prefix: Vec::new(),
        longer: false,
        guards: Guards::new(),
    }])
}

impl Word {
    pub fn display(&self) -> String {
        let mut parts = self
            .prefix
            .iter()
            .map(|tag| match tag {
                Tag::Named(name) => name.clone(),
                Tag::Any | Tag::Parameter(..) | Tag::FutureElement(..) => "Error".into(),
            })
            .collect::<Vec<_>>();
        if self.longer {
            parts.push("...".into());
        }
        format!("[{}]", parts.join(", "))
    }
}

pub(crate) fn one(tag: Tag) -> Language {
    let guards = if let Tag::FutureElement(input, _) = &tag {
        BTreeMap::from([(input.clone(), Guard::Future(None))])
    } else {
        Guards::new()
    };
    BTreeSet::from([Word {
        prefix: vec![tag],
        longer: false,
        guards,
    }])
}

pub(crate) fn unknown(bound: usize) -> Language {
    let mut result = (1..=bound)
        .map(|len| Word {
            prefix: vec![Tag::Any; len],
            longer: false,
            guards: Guards::new(),
        })
        .collect::<Language>();
    result.insert(Word {
        prefix: vec![Tag::Any; bound],
        longer: true,
        guards: Guards::new(),
    });
    result
}

pub(super) fn future(input: Input, bound: usize) -> Language {
    unknown(bound)
        .into_iter()
        .map(|mut word| {
            word.prefix = (0..word.prefix.len())
                .map(|index| Tag::FutureElement(input.clone(), index))
                .collect();
            if element_input(&input) {
                let mut capability = input.clone();
                for projection in &mut capability.path {
                    if let super::Projection::Element(ty, _) | super::Projection::AllElements(ty) =
                        projection
                    {
                        *projection = super::Projection::AnyFailure(ty.clone());
                    }
                }
                capability.path.push(super::Projection::FailureShape(
                    word.prefix.len(),
                    word.longer,
                ));
                word.guards.insert(capability, Guard::Future(None));
            } else {
                word.guards.insert(
                    input.clone(),
                    Guard::Future(Some(Shape {
                        length: word.prefix.len(),
                        longer: word.longer,
                    })),
                );
            }
            word
        })
        .collect()
}
pub(super) fn merge_guards(left: &Guards, right: &Guards) -> Option<Guards> {
    let mut merged = left.clone();
    for (input, guard) in right {
        match (merged.get(input), guard) {
            (None, _) => {
                merged.insert(input.clone(), guard.clone());
            }
            (Some(previous), next) if previous == next => {}
            (Some(Guard::Future(Some(_))), Guard::Future(None)) => {}
            (Some(Guard::Future(None)), Guard::Future(Some(_))) => {
                merged.insert(input.clone(), guard.clone());
            }
            _ => return None,
        }
    }
    Some(merged)
}
pub(super) fn restricted(errors: &Language, conditions: &Guards) -> Language {
    errors
        .iter()
        .filter_map(|word| {
            Some(Word {
                prefix: word.prefix.clone(),
                longer: word.longer,
                guards: merge_guards(&word.guards, conditions)?,
            })
        })
        .collect()
}
pub(super) fn conditioned(errors: &Language, paths: &Language) -> Language {
    let mut result = Language::new();
    for error in errors {
        for path in paths {
            if let Some(guards) = merge_guards(&error.guards, &path.guards) {
                result.insert(Word {
                    prefix: error.prefix.clone(),
                    longer: error.longer,
                    guards,
                });
            }
        }
    }
    result
}

pub(crate) fn concatenate(left: &Language, right: &Language, bound: usize) -> Language {
    let mut result = Language::new();
    for a in left {
        for b in right {
            let Some(guards) = merge_guards(&a.guards, &b.guards) else {
                continue;
            };
            let mut prefix = a.prefix.clone();
            if !a.longer {
                prefix.extend(
                    b.prefix
                        .iter()
                        .take(bound.saturating_sub(prefix.len()))
                        .cloned(),
                );
            }
            result.insert(Word {
                prefix,
                longer: a.longer || b.longer || a.prefix.len() + b.prefix.len() > bound,
                guards,
            });
        }
    }
    result
}

#[derive(Clone, Debug)]
pub(crate) struct Pattern {
    pub prefix: Vec<Tag>,
    pub rest: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum Match {
    Never,
    Some,
    All,
}

impl Pattern {
    pub fn whole() -> Self {
        Self {
            prefix: Vec::new(),
            rest: true,
        }
    }
    pub fn matches(&self, word: &Word) -> Match {
        if (!self.rest && (word.longer || word.prefix.len() != self.prefix.len()))
            || word.prefix.len() < self.prefix.len()
        {
            return Match::Never;
        }
        let mut result = Match::All;
        for (expected, actual) in self.prefix.iter().zip(&word.prefix) {
            if *expected == Tag::Any || expected == actual {
                continue;
            }
            match actual {
                Tag::Any | Tag::Parameter(..) | Tag::FutureElement(..) => result = Match::Some,
                Tag::Named(_) => return Match::Never,
            }
        }
        result
    }
    /// A finite positive set of nominal tests cannot exhaust an open Error
    /// position unless one test accepts that whole position. Splitting by
    /// length also catches unions such as [A] followed by [A, Error, ...].
    pub fn covered_by(&self, previous: &[Self], bound: usize) -> bool {
        let mut witnesses = Language::new();
        if self.rest {
            for len in self.prefix.len().max(1)..=bound {
                let mut prefix = self.prefix.clone();
                prefix.resize(len, Tag::Any);
                witnesses.insert(Word {
                    prefix,
                    longer: false,
                    guards: Guards::new(),
                });
            }
            let mut prefix = self.prefix.clone();
            prefix.resize(bound, Tag::Any);
            witnesses.insert(Word {
                prefix,
                longer: true,
                guards: Guards::new(),
            });
        } else {
            witnesses.insert(Word {
                prefix: self.prefix.clone(),
                longer: false,
                guards: Guards::new(),
            });
        }
        witnesses.iter().all(|word| {
            previous
                .iter()
                .any(|pattern| pattern.matches(word) == Match::All)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(names: &[&str], bound: usize) -> Word {
        Word {
            prefix: names
                .iter()
                .take(bound)
                .map(|name| Tag::Named((*name).into()))
                .collect(),
            longer: names.len() > bound,
            guards: Guards::new(),
        }
    }

    #[test]
    fn truncated_concatenation_agrees_with_concrete_words() {
        for bound in 1..=4 {
            for a in 1..=6 {
                for b in 1..=6 {
                    let left = vec!["A"; a];
                    let right = vec!["B"; b];
                    let expected = word(
                        &left.iter().chain(&right).copied().collect::<Vec<_>>(),
                        bound,
                    );
                    assert_eq!(
                        concatenate(
                            &BTreeSet::from([word(&left, bound)]),
                            &BTreeSet::from([word(&right, bound)]),
                            bound
                        ),
                        BTreeSet::from([expected])
                    );
                }
            }
        }
    }

    #[test]
    fn order_correlations_are_not_replaced_by_independent_position_unions() {
        let errors = BTreeSet::from([word(&["A", "B"], 2), word(&["B", "A"], 2)]);
        let patterns = [
            Pattern {
                prefix: vec![Tag::Named("A".into()), Tag::Named("B".into())],
                rest: false,
            },
            Pattern {
                prefix: vec![Tag::Named("B".into()), Tag::Named("A".into())],
                rest: false,
            },
        ];
        assert!(errors.iter().all(|word| {
            patterns
                .iter()
                .any(|pattern| pattern.matches(word) == Match::All)
        }));
        assert!(
            patterns
                .iter()
                .all(|pattern| pattern.matches(&word(&["A", "A"], 2)) == Match::Never)
        );
    }
}
