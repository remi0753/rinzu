//! One ordered witness per semantic word, collected only for diagnostics.
use super::{Origin, OriginKind, Tag, Word, merge_guards};
use crate::SourceRange;
use std::sync::Arc;

pub(in crate::error_effects) const VISIBLE_ERRORS: usize = 8;

#[derive(Clone, Copy, Debug)]
pub(in crate::error_effects) enum Count {
    Exact(usize),
    AtLeast(usize),
}

impl Count {
    pub fn of(word: &Word) -> Self {
        if word.longer {
            Self::AtLeast(word.prefix.len() + 1)
        } else {
            Self::Exact(word.prefix.len())
        }
    }

    pub fn add(self, other: Self) -> Self {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => Self::Exact(a.saturating_add(b)),
            (Self::Exact(a) | Self::AtLeast(a), Self::Exact(b) | Self::AtLeast(b)) => {
                Self::AtLeast(a.saturating_add(b))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::error_effects) struct Reception {
    pub label: String,
    pub callee: Option<String>,
    pub contract: Option<SourceRange>,
    pub fresh_task: bool,
}

#[derive(Clone, Debug)]
pub(in crate::error_effects) enum PartKind {
    Element(Tag),
    Received(Word, Option<Arc<Reception>>),
    Observation,
    Unordered,
}

#[derive(Clone, Debug)]
pub(in crate::error_effects) struct Part {
    pub kind: PartKind,
    pub origin: Option<Origin>,
    pub cleanup: Option<SourceRange>,
}

#[derive(Clone, Debug)]
pub(in crate::error_effects) struct Sequence {
    // Normalization can widen the key in Language. This is the projection
    // of this particular sequence, before the widening.
    pub word: Word,
    pub count: Option<Count>,
    pub parts: Vec<Part>,
    pub omitted: bool,
    awaits: bool,
    may_alias_await: bool,
}

impl Sequence {
    pub(super) fn at(word: &Word, origin: Origin, reception: Option<Arc<Reception>>) -> Self {
        let awaits = origin.kind == OriginKind::Await;
        let may_alias_await = awaits && reception.as_ref().is_none_or(|site| !site.fresh_task);
        let kind = if matches!(origin.kind, OriginKind::Throw | OriginKind::Operation)
            && !word.longer
            && word.prefix.len() == 1
        {
            PartKind::Element(word.prefix[0].clone())
        } else {
            PartKind::Received(word.clone(), reception)
        };
        Self {
            word: word.clone(),
            count: Some(Count::of(word)),
            parts: vec![Part {
                kind,
                origin: Some(origin),
                cleanup: None,
            }],
            omitted: false,
            awaits,
            may_alias_await,
        }
    }

    pub(super) fn unknown(word: &Word, cleanup: Option<SourceRange>) -> Self {
        Self {
            word: word.clone(),
            count: None,
            parts: vec![Part {
                kind: PartKind::Unordered,
                origin: None,
                cleanup,
            }],
            omitted: false,
            awaits: false,
            may_alias_await: false,
        }
    }

    pub(super) fn restrict(mut self, upper: &Word) -> Option<Self> {
        self.word.guards = merge_guards(&self.word.guards, &upper.guards)?;
        upper.contains(&self.word).then_some(self)
    }

    pub(super) fn mark_cleanup(&mut self, range: SourceRange) {
        for part in &mut self.parts {
            part.cleanup.get_or_insert(range);
        }
    }

    pub(super) fn join(
        a: &Word,
        b: &Word,
        left: Option<&Self>,
        right: Option<&Self>,
        bound: usize,
        independent: bool,
    ) -> Option<Self> {
        if left.is_none() && right.is_none() {
            return None;
        }
        let a_word = left.map_or(a, |seq| &seq.word);
        let b_word = right.map_or(b, |seq| &seq.word);
        let guards = merge_guards(&a_word.guards, &b_word.guards)?;
        let empty = |word: &Word| word.prefix.is_empty() && !word.longer;
        if empty(a) {
            return right.cloned().map(|mut seq| {
                seq.word.guards = guards;
                seq
            });
        }
        if empty(b) && independent {
            return left.cloned().map(|mut seq| {
                seq.word.guards = guards;
                seq
            });
        }

        let awaits = left.is_some_and(|s| s.awaits) || right.is_some_and(|s| s.awaits);
        let may_alias_await =
            left.is_some_and(|s| s.may_alias_await) || right.is_some_and(|s| s.may_alias_await);
        let repeated_await =
            may_alias_await && left.is_some_and(|s| s.awaits) && right.is_some_and(|s| s.awaits);
        let connect = independent && !repeated_await;
        let mut joined = left.cloned().unwrap_or_else(|| Self::unknown(a, None));
        joined.word = a_word.concatenate(if connect { b_word } else { b }, guards, bound);
        joined.count = match (joined.count, right.and_then(|s| s.count)) {
            (Some(a), Some(b)) if connect => Some(a.add(b)),
            _ => None,
        };
        joined.awaits = awaits;
        joined.may_alias_await = may_alias_await;
        let mut right = right.cloned().unwrap_or_else(|| Self::unknown(b, None));
        if !connect {
            // The continuation may depend on a merged local, or may be another
            // observation of the same Future. Keep sites without numbering it.
            right.parts.retain_mut(|part| {
                if part
                    .origin
                    .is_some_and(|origin| origin.kind == OriginKind::Await)
                {
                    part.kind = PartKind::Observation;
                    true
                } else if part.cleanup.is_some() {
                    part.kind = PartKind::Unordered;
                    part.origin = None;
                    true
                } else {
                    false
                }
            });
            right.omitted = true;
        }
        let capacity = VISIBLE_ERRORS.saturating_sub(joined.parts.len());
        joined.omitted |= right.omitted || right.parts.len() > capacity;
        joined.parts.extend(right.parts.into_iter().take(capacity));
        Some(joined)
    }

    pub fn precedes(&self, other: &Self) -> bool {
        let key = |part: &Part| part.origin.map(Origin::order);
        self.precision()
            .cmp(&other.precision())
            .reverse()
            .then_with(|| self.word.cmp(&other.word))
            .then_with(|| self.parts.iter().map(key).cmp(other.parts.iter().map(key)))
            .is_lt()
    }

    fn precision(&self) -> u8 {
        if matches!(self.count, Some(Count::Exact(_)))
            && self
                .parts
                .iter()
                .all(|p| matches!(p.kind, PartKind::Element(Tag::Named(_))))
        {
            3
        } else if self.parts.iter().any(|p| {
            matches!(
                &p.kind,
                PartKind::Element(Tag::Named(_)) | PartKind::Received(_, Some(_))
            )
        }) {
            2
        } else if self.parts.iter().any(|p| p.origin.is_some()) {
            1
        } else {
            0
        }
    }
}
