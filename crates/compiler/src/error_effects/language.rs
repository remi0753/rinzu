//! Finite observations of error words. Catch patterns inspect only a bounded
//! prefix and exact length, so retaining that prefix plus a longer-word bit is
//! sufficient even for recursive propagation and repeated cleanup failures.
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;

mod origins;
mod sequence;
use origins::Origins;
pub(super) use origins::{Kind as OriginKind, Origin, outside};
pub(super) use sequence::{Count, Part, PartKind, Reception, Sequence, VISIBLE_ERRORS};

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
    CaughtType(String),
}
pub(super) type Guards = BTreeMap<Input, Guard>;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum Tag {
    Any,
    Named(String),
    Set(BTreeSet<String>),
    Parameter(Input),
    FutureElement(Input, usize),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Word {
    pub prefix: Vec<Tag>,
    /// All possible tags beyond the observable prefix. Count/order are erased,
    /// but declared Error bounds must still check every cleanup/collected Error.
    pub tail: BTreeSet<Tag>,
    pub longer: bool,
    pub(super) guards: Guards,
}

/// The semantic set alone determines equality, hashing and convergence.
/// Only the diagnostic recheck attaches a witness map to this set.
#[derive(Clone, Debug, Default)]
pub(crate) struct Language {
    words: BTreeSet<Word>,
    trace: Option<Box<Trace>>,
}

#[derive(Clone, Debug)]
struct Trace {
    allowed: Arc<BTreeSet<Tag>>,
    witnesses: BTreeMap<Word, Witness>,
}

#[derive(Clone, Debug)]
struct Witness {
    origins: Origins,
    sequence: Option<Sequence>,
}

impl Witness {
    fn merge(&mut self, other: Self, prefix: &[Tag], allowed: &BTreeSet<Tag>) {
        self.origins.merge(other.origins, prefix, allowed);
        if let Some(next) = other.sequence
            && self
                .sequence
                .as_ref()
                .is_none_or(|current| next.precedes(current))
        {
            self.sequence = Some(next);
        }
    }
}

impl Language {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, word: Word) -> bool {
        self.words.insert(word)
    }

    pub fn clear(&mut self) {
        self.words.clear();
        self.trace = None;
    }

    pub fn extend(&mut self, other: Self) {
        if let Some(trace) = other.trace {
            for (word, witness) in trace.witnesses {
                self.record(&trace.allowed, &word, witness);
            }
        }
        self.words.extend(other.words);
    }

    pub(super) fn at(
        mut self,
        allowed: &Arc<BTreeSet<Tag>>,
        origin: Origin,
        ordered: bool,
        reception: Option<Arc<Reception>>,
    ) -> Self {
        if !self.words.is_empty() {
            let witnesses = self
                .words
                .iter()
                .map(|word| {
                    (
                        word.clone(),
                        Witness {
                            origins: Origins::at(word, origin),
                            sequence: ordered
                                .then(|| Sequence::at(word, origin, reception.clone())),
                        },
                    )
                })
                .collect();
            self.trace = Some(Box::new(Trace {
                allowed: allowed.clone(),
                witnesses,
            }));
        }
        self
    }

    pub(super) fn origins(&self, word: &Word) -> Option<&Origins> {
        Some(&self.witness(word)?.origins)
    }

    pub(super) fn sequence(&self, word: &Word) -> Option<&Sequence> {
        self.witness(word)?.sequence.as_ref()
    }

    fn witness(&self, word: &Word) -> Option<&Witness> {
        self.trace.as_ref()?.witnesses.get(word)
    }

    pub(super) fn mark_cleanup(&mut self, range: crate::SourceRange, independent: bool) {
        if let Some(trace) = &mut self.trace {
            for (word, witness) in &mut trace.witnesses {
                if let Some(sequence) = &mut witness.sequence {
                    if !independent {
                        *sequence = Sequence::unknown(word, Some(range));
                    } else {
                        sequence.mark_cleanup(range);
                    }
                }
            }
        }
    }

    fn record(&mut self, allowed: &Arc<BTreeSet<Tag>>, word: &Word, witness: Witness) {
        let trace = self.trace.get_or_insert_with(|| {
            Box::new(Trace {
                allowed: allowed.clone(),
                witnesses: BTreeMap::new(),
            })
        });
        match trace.witnesses.entry(word.clone()) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().merge(witness, &word.prefix, allowed);
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(witness);
            }
        }
    }

    fn insert_from(&mut self, next: Word, source: &Self, previous: &Word) {
        if let Some(trace) = &source.trace
            && let Some(witness) = trace.witnesses.get(previous)
        {
            let mut witness = witness.clone();
            witness.sequence = witness
                .sequence
                .and_then(|sequence| sequence.restrict(&next));
            self.record(&trace.allowed, &next, witness);
        }
        self.insert(next);
    }

    pub(super) fn retain(&mut self, keep: impl Fn(&Word) -> bool) {
        self.words.retain(&keep);
        if let Some(trace) = &mut self.trace {
            trace.witnesses.retain(|word, _| keep(word));
        }
    }
}

impl PartialEq for Language {
    fn eq(&self, other: &Self) -> bool {
        self.words == other.words
    }
}
impl Eq for Language {}
impl PartialOrd for Language {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Language {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.words.cmp(&other.words)
    }
}
impl Hash for Language {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.words.hash(state);
    }
}
impl Deref for Language {
    type Target = BTreeSet<Word>;
    fn deref(&self) -> &Self::Target {
        &self.words
    }
}
impl IntoIterator for Language {
    type Item = Word;
    type IntoIter = std::collections::btree_set::IntoIter<Word>;
    fn into_iter(self) -> Self::IntoIter {
        self.words.into_iter()
    }
}
impl<'a> IntoIterator for &'a Language {
    type Item = &'a Word;
    type IntoIter = std::collections::btree_set::Iter<'a, Word>;
    fn into_iter(self) -> Self::IntoIter {
        self.words.iter()
    }
}
impl FromIterator<Word> for Language {
    fn from_iter<T: IntoIterator<Item = Word>>(iter: T) -> Self {
        Self {
            words: iter.into_iter().collect(),
            trace: None,
        }
    }
}
impl<const N: usize> From<[Word; N]> for Language {
    fn from(words: [Word; N]) -> Self {
        Self {
            words: BTreeSet::from(words),
            trace: None,
        }
    }
}

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
    let independent = |tag: &Tag| match tag {
        Tag::FutureElement(input, _) | Tag::Parameter(input) if element_input(input) => Tag::Any,
        tag => tag.clone(),
    };
    let mut result = Language::new();
    for word in errors {
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
                    if let super::Projection::Element(ty, _) | super::Projection::AllElements(ty) =
                        projection
                    {
                        *projection = super::Projection::AnyFailure(ty.clone());
                    }
                }
                Some((input, Guard::Future(None)))
            })
            .collect();
        let next = Word {
            prefix: word.prefix.iter().map(independent).collect(),
            tail: word.tail.iter().map(independent).collect(),
            longer: word.longer,
            guards,
        };
        if let Some(trace) = &errors.trace
            && let Some(witness) = trace.witnesses.get(word)
        {
            result.record(
                &trace.allowed,
                &next,
                Witness {
                    origins: witness.origins.remap_tail(independent),
                    // A loop may visit a different element on each iteration.
                    sequence: witness
                        .sequence
                        .as_ref()
                        .map(|_| Sequence::unknown(&next, None)),
                },
            );
        }
        result.insert(next);
    }
    result
}

pub(crate) fn empty_word() -> Language {
    Language::from([Word {
        tail: BTreeSet::new(),
        prefix: Vec::new(),
        longer: false,
        guards: Guards::new(),
    }])
}

impl Word {
    fn with_guards(&self, guards: Guards) -> Self {
        Self {
            prefix: self.prefix.clone(),
            tail: self.tail.clone(),
            longer: self.longer,
            guards,
        }
    }

    fn concatenate(&self, right: &Self, guards: Guards, bound: usize) -> Self {
        let available = self.available(bound);
        let mut prefix = self.prefix.clone();
        prefix.extend(right.prefix.iter().take(available).cloned());
        let mut tail = self.tail.clone();
        tail.extend(right.tail.iter().cloned());
        tail.extend(right.prefix.iter().skip(available).cloned());
        Self {
            prefix,
            tail,
            longer: self.longer || right.longer || self.prefix.len() + right.prefix.len() > bound,
            guards,
        }
    }

    fn available(&self, bound: usize) -> usize {
        if self.longer {
            0
        } else {
            bound.saturating_sub(self.prefix.len())
        }
    }

    pub(super) fn contains(&self, other: &Self) -> bool {
        fn includes(upper: &Tag, actual: &Tag) -> bool {
            match (upper, actual) {
                (Tag::Any, _) => true,
                (Tag::Set(names), Tag::Named(name)) => names.contains(name),
                (Tag::Set(names), Tag::Set(actual)) => actual.is_subset(names),
                _ => upper == actual,
            }
        }
        self.longer == other.longer
            && self.prefix.len() == other.prefix.len()
            && self
                .prefix
                .iter()
                .zip(&other.prefix)
                .all(|(a, b)| includes(a, b))
            && other
                .tail
                .iter()
                .all(|b| self.tail.iter().any(|a| includes(a, b)))
            && self.guards.iter().all(|(input, guard)| {
                other.guards.get(input).is_some_and(|actual| {
                    actual == guard
                        || matches!(
                            (guard, actual),
                            (Guard::Future(None), Guard::Future(Some(_)))
                        )
                })
            })
    }
}

pub(crate) fn one(tag: Tag) -> Language {
    let guards = if let Tag::FutureElement(input, _) = &tag {
        BTreeMap::from([(input.clone(), Guard::Future(None))])
    } else {
        Guards::new()
    };
    Language::from([Word {
        tail: BTreeSet::new(),
        prefix: vec![tag],
        longer: false,
        guards,
    }])
}

pub(crate) fn unknown(bound: usize) -> Language {
    let mut result = (1..=bound)
        .map(|len| Word {
            tail: BTreeSet::new(),
            prefix: vec![Tag::Any; len],
            longer: false,
            guards: Guards::new(),
        })
        .collect::<Language>();
    result.insert(Word {
        tail: BTreeSet::from([Tag::Any]),
        prefix: vec![Tag::Any; bound],
        longer: true,
        guards: Guards::new(),
    });
    result
}

/// Subtract a catch without enumerating the Cartesian product of finite Error
/// sets at every position. Open Error/parameter positions remain conservative.
pub(super) fn subtract(errors: &Language, pattern: &Pattern) -> Language {
    let mut result = Language::new();
    for word in errors {
        if pattern.matches(word) == Match::Never {
            result.insert_from(word.clone(), errors, word);
            continue;
        }
        let mut inside = word.clone();
        let mut open = false;
        for (index, expected) in pattern.prefix.iter().enumerate() {
            if *expected == Tag::Any {
                continue;
            }
            match (&inside.prefix[index], expected) {
                (Tag::Set(names), Tag::Named(name)) if names.contains(name) => {
                    let mut rest = names.clone();
                    rest.remove(name);
                    if !rest.is_empty() {
                        let mut outside = inside.clone();
                        outside.prefix[index] = if rest.len() == 1 {
                            Tag::Named(rest.into_iter().next().unwrap())
                        } else {
                            Tag::Set(rest)
                        };
                        result.insert_from(outside, errors, word);
                    }
                    inside.prefix[index] = expected.clone();
                }
                (actual, _) if actual == expected => {}
                _ => {
                    open = true;
                }
            }
        }
        if open {
            result.insert_from(inside, errors, word);
        }
    }
    result
}

/// Preserve the incoming guard when a catch narrows a symbolic Error.
pub(super) fn narrow_tag(word: &mut Word, index: usize, expected: &Tag) -> bool {
    let actual = &word.prefix[index];
    match Pattern::match_tag(expected, actual) {
        Match::Never => return false,
        Match::All => return true,
        Match::Some => {}
    }
    if let Tag::Named(name) = expected {
        let input = match actual {
            Tag::Parameter(input) => Some(input.clone()),
            Tag::FutureElement(input, element) => {
                let mut input = input.clone();
                input.path.push(super::Projection::FailureElement(*element));
                Some(input)
            }
            _ => None,
        };
        if let Some(input) = input {
            let condition = Guards::from([(input, Guard::CaughtType(name.clone()))]);
            let Some(guards) = merge_guards(&word.guards, &condition) else {
                return false;
            };
            word.guards = guards;
        }
    }
    word.prefix[index] = expected.clone();
    true
}

pub(super) fn accepted(errors: &Language, pattern: &Pattern) -> Language {
    let mut accepted = Language::new();
    for word in errors {
        if pattern.length_mismatch(word).is_none() {
            let mut narrowed = word.clone();
            if pattern
                .prefix
                .iter()
                .enumerate()
                .all(|(index, tag)| narrow_tag(&mut narrowed, index, tag))
            {
                accepted.insert_from(narrowed, errors, word);
            }
        }
    }
    accepted
}

pub(super) fn select_caught(
    errors: &Language,
    elements: &[u32],
    tail: Option<u32>,
    bound: usize,
    forwarded: Option<Origin>,
) -> Language {
    let mut selected = Language::new();
    for word in errors {
        let positions = elements
            .iter()
            .map(|index| *index as usize)
            .chain(
                tail.into_iter()
                    .flat_map(|start| start as usize..word.prefix.len()),
            )
            .collect::<Vec<_>>();
        let prefix = positions
            .iter()
            .map(|index| word.prefix[*index].clone())
            .collect::<Vec<_>>();
        let mut add = |prefix: Vec<Tag>, longer: bool| {
            let next = Word {
                prefix,
                longer,
                tail: if longer {
                    word.tail.clone()
                } else {
                    BTreeSet::new()
                },
                guards: word.guards.clone(),
            };
            if let Some(forwarded) = forwarded
                && let Some(trace) = &errors.trace
            {
                let origins = errors.origins(word);
                let project = |origin: Option<Origin>| {
                    let mut origin = origin.unwrap_or(forwarded);
                    origin.forwarded = Some(forwarded.range);
                    Some(origin)
                };
                let origins = Origins {
                    prefix: (0..next.prefix.len())
                        .map(|i| {
                            project(positions.get(i).and_then(|position| {
                                origins.and_then(|origins| origins.prefix[*position])
                            }))
                        })
                        .collect(),
                    tail: next
                        .tail
                        .iter()
                        .map(|tag| {
                            (
                                tag.clone(),
                                project(origins.and_then(|origins| origins.tail.get(tag).copied()))
                                    .unwrap(),
                            )
                        })
                        .collect(),
                };
                let sequence = errors
                    .sequence(word)
                    .map(|_| Sequence::selected(&next, &origins));
                selected.record(&trace.allowed, &next, Witness { origins, sequence });
            }
            selected.insert(next);
        };
        if tail.is_some() && word.longer {
            // The old tail guarantees at least one further Error. Every
            // shorter exact length and the new open tail remain possible.
            let mut names = BTreeSet::new();
            let finite = word.tail.iter().all(|tag| match tag {
                Tag::Named(name) => {
                    names.insert(name.clone());
                    true
                }
                Tag::Set(set) => {
                    names.extend(set.clone());
                    true
                }
                _ => false,
            });
            let tag = if finite && !names.is_empty() {
                Tag::Set(names)
            } else {
                Tag::Any
            };
            for length in prefix.len() + 1..=bound {
                let mut exact = prefix.clone();
                exact.resize(length, tag.clone());
                add(exact, false);
            }
            let mut longer = prefix;
            longer.resize(bound, tag);
            add(longer, true);
        } else if !prefix.is_empty() {
            add(prefix, false);
        }
    }
    selected
}

pub(super) fn future(input: Input, bound: usize) -> Language {
    unknown(bound)
        .into_iter()
        .map(|mut word| {
            if word.longer {
                word.tail = BTreeSet::from([Tag::FutureElement(input.clone(), bound)]);
            }
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
    let mut result = Language::new();
    for word in errors {
        if let Some(guards) = merge_guards(&word.guards, conditions) {
            result.insert_from(word.with_guards(guards), errors, word);
        }
    }
    result
}
pub(super) fn conditioned(errors: &Language, paths: &Language) -> Language {
    let mut result = Language::new();
    for error in errors {
        for path in paths {
            if let Some(guards) = merge_guards(&error.guards, &path.guards) {
                result.insert_from(error.with_guards(guards), errors, error);
            }
        }
    }
    result
}

pub(crate) fn concatenate(left: &Language, right: &Language, bound: usize) -> Language {
    concatenate_with_evidence(left, right, bound, true)
}

pub(super) fn concatenate_with_evidence(
    left: &Language,
    right: &Language,
    bound: usize,
    independent: bool,
) -> Language {
    let mut result = Language::new();
    let trace = left.trace.as_ref().or(right.trace.as_ref());
    for a in left {
        for b in right {
            let Some(guards) = merge_guards(&a.guards, &b.guards) else {
                continue;
            };
            let word = a.concatenate(b, guards, bound);
            if let Some(trace) = trace {
                let left = left.witness(a);
                let right = right.witness(b);
                if left.is_some() || right.is_some() {
                    result.record(
                        &trace.allowed,
                        &word,
                        Witness {
                            origins: Origins::concatenate(
                                a,
                                b,
                                left.map(|w| &w.origins),
                                right.map(|w| &w.origins),
                                a.available(bound),
                            ),
                            sequence: Sequence::join(
                                a,
                                b,
                                left.and_then(|w| w.sequence.as_ref()),
                                right.and_then(|w| w.sequence.as_ref()),
                                bound,
                                independent,
                            ),
                        },
                    );
                }
            }
            result.insert(word);
        }
    }
    normalize(result)
}

/// Tail order is unobservable. Merge its type sets for identical shape/guards
/// so collecting many optional failures does not enumerate all suffix subsets.
pub(super) fn normalize(errors: Language) -> Language {
    let mut words = BTreeMap::<(Vec<Tag>, bool, Guards), BTreeSet<Tag>>::new();
    let mut trace = errors.trace;
    let mut witnesses = BTreeMap::<(Vec<Tag>, bool, Guards), Witness>::new();
    for word in errors.words {
        if let Some(trace) = &mut trace
            && let Some(witness) = trace.witnesses.remove(&word)
        {
            let shape = (word.prefix.clone(), word.longer, word.guards.clone());
            match witnesses.entry(shape) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().merge(witness, &word.prefix, &trace.allowed);
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(witness);
                }
            }
        }
        words
            .entry((word.prefix, word.longer, word.guards))
            .or_default()
            .extend(word.tail);
    }
    let mut result = Language::new();
    for (shape, tail) in words {
        let origins = trace.as_ref().and_then(|trace| {
            witnesses
                .remove(&shape)
                .map(|origins| (&trace.allowed, origins))
        });
        let (prefix, longer, guards) = shape;
        let word = Word {
            prefix,
            longer,
            guards,
            tail,
        };
        if let Some((allowed, origins)) = origins {
            result.record(allowed, &word, origins);
        }
        result.insert(word);
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

#[derive(Clone, Copy)]
pub(super) enum Mismatch {
    TooLong,
    TooShort,
    Type(usize),
    Open(usize),
}

impl Pattern {
    pub fn whole() -> Self {
        Self {
            prefix: Vec::new(),
            rest: true,
        }
    }
    fn length_mismatch(&self, word: &Word) -> Option<Mismatch> {
        if !self.rest && (word.longer || word.prefix.len() > self.prefix.len()) {
            Some(Mismatch::TooLong)
        } else if word.prefix.len() < self.prefix.len() {
            Some(Mismatch::TooShort)
        } else {
            None
        }
    }

    pub(super) fn match_tag(expected: &Tag, actual: &Tag) -> Match {
        if *expected == Tag::Any || expected == actual {
            return Match::All;
        }
        match actual {
            Tag::Set(names) => match expected {
                Tag::Named(name) if !names.contains(name) => Match::Never,
                Tag::Named(_) if names.len() == 1 => Match::All,
                _ => Match::Some,
            },
            Tag::Any | Tag::Parameter(..) | Tag::FutureElement(..) => Match::Some,
            Tag::Named(_) => Match::Never,
        }
    }

    pub fn matches(&self, word: &Word) -> Match {
        if self.length_mismatch(word).is_some() {
            return Match::Never;
        }
        let mut result = Match::All;
        for (expected, actual) in self.prefix.iter().zip(&word.prefix) {
            match Self::match_tag(expected, actual) {
                Match::Never => return Match::Never,
                Match::Some => result = Match::Some,
                Match::All => {}
            }
        }
        result
    }

    pub(super) fn mismatch(&self, word: &Word) -> Option<Mismatch> {
        if let Some(length) = self.length_mismatch(word) {
            return Some(length);
        }
        let mut open = None;
        for (i, (expected, actual)) in self.prefix.iter().zip(&word.prefix).enumerate() {
            match Self::match_tag(expected, actual) {
                Match::Never => return Some(Mismatch::Type(i)),
                Match::Some => {
                    open.get_or_insert(Mismatch::Open(i));
                }
                Match::All => {}
            }
        }
        open
    }
    /// A finite positive set of nominal tests cannot exhaust an open Error
    /// position unless one test accepts that whole position. Splitting by
    /// length also catches unions such as [A] followed by [A, Error, ...].
    pub(super) fn covered_by(
        &self,
        previous: &[Self],
        bound: usize,
    ) -> Option<Vec<(usize, Option<Shape>)>> {
        if previous.is_empty() {
            return None;
        }
        let mut witnesses = Language::new();
        if self.rest {
            for len in self.prefix.len().max(1)..=bound {
                let mut prefix = self.prefix.clone();
                prefix.resize(len, Tag::Any);
                witnesses.insert(Word {
                    prefix,
                    tail: BTreeSet::new(),
                    longer: false,
                    guards: Guards::new(),
                });
            }
            let mut prefix = self.prefix.clone();
            prefix.resize(bound, Tag::Any);
            witnesses.insert(Word {
                prefix,
                tail: BTreeSet::from([Tag::Any]),
                longer: true,
                guards: Guards::new(),
            });
        } else {
            witnesses.insert(Word {
                tail: BTreeSet::new(),
                prefix: self.prefix.clone(),
                longer: false,
                guards: Guards::new(),
            });
        }
        let covering = |word: &Word| {
            previous
                .iter()
                .position(|pattern| pattern.matches(word) == Match::All)
        };
        if !witnesses.iter().all(|word| covering(word).is_some()) {
            return None;
        }
        if let Some(index) = previous.iter().position(|pattern| {
            witnesses
                .iter()
                .all(|word| pattern.matches(word) == Match::All)
        }) {
            return Some(vec![(index, None)]);
        }
        Some(
            witnesses
                .iter()
                .map(|word| {
                    (
                        covering(word).unwrap(),
                        Some(Shape {
                            length: word.prefix.len(),
                            longer: word.longer,
                        }),
                    )
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(names: &[&str], bound: usize) -> Word {
        Word {
            tail: names
                .iter()
                .skip(bound)
                .map(|name| Tag::Named((*name).into()))
                .collect(),
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
                            &Language::from([word(&left, bound)]),
                            &Language::from([word(&right, bound)]),
                            bound
                        ),
                        Language::from([expected])
                    );
                }
            }
        }
    }

    #[test]
    fn witness_merging_keeps_one_prefix_and_does_not_change_the_error_language() {
        let a = Tag::Named("A".into());
        let b = Tag::Named("B".into());
        let allowed = Arc::new(BTreeSet::from([a.clone()]));
        let traced = |tag, byte| {
            one(tag).at(
                &allowed,
                Origin {
                    range: crate::SourceRange::new(crate::FileId(1), byte, byte + 1),
                    kind: OriginKind::Throw,
                    forwarded: None,
                    in_finally: false,
                },
                true,
                None,
            )
        };
        for bound in [1, 2] {
            let expected = concatenate(&one(a.clone()), &one(b.clone()), bound);
            let mut errors = concatenate(&traced(a.clone(), 12), &traced(b.clone(), 40), bound);
            errors.extend(concatenate(
                &traced(a.clone(), 16),
                &traced(b.clone(), 28),
                bound,
            ));
            let errors = normalize(errors);
            assert_eq!(errors, expected);
            let origins = errors.origins(errors.first().unwrap()).unwrap();
            if bound == 1 {
                assert_eq!(origins.prefix[0].unwrap().range.start_byte, 12);
                assert_eq!(origins.tail[&b].range.start_byte, 28);
            } else {
                assert_eq!(origins.prefix[0].unwrap().range.start_byte, 16);
                assert_eq!(origins.prefix[1].unwrap().range.start_byte, 28);
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
