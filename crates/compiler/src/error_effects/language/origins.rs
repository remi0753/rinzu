//! Witnesses for the bounded prefix and the unordered tail of an Error word.
use super::{Tag, Word};
use crate::SourceRange;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::error_effects) enum Kind {
    Call,
    UnknownCallback,
    Await,
    Throw,
    Operation,
}

#[derive(Clone, Copy, Debug)]
pub(in crate::error_effects) struct Origin {
    pub range: SourceRange,
    pub kind: Kind,
    pub in_finally: bool,
}

impl Origin {
    pub(super) fn order(self) -> (u32, u32, u32, Kind) {
        (
            self.range.file_id.0,
            self.range.start_byte,
            self.range.end_byte,
            self.kind,
        )
    }
}

pub(in crate::error_effects) fn outside(tag: &Tag, allowed: &BTreeSet<Tag>) -> bool {
    match tag {
        Tag::Named(_) => !allowed.contains(tag),
        Tag::Set(names) => names
            .iter()
            .any(|name| !allowed.contains(&Tag::Named(name.clone()))),
        Tag::Any | Tag::Parameter(_) | Tag::FutureElement(_, _) => true,
    }
}

#[derive(Clone, Debug)]
pub(in crate::error_effects) struct Origins {
    pub prefix: Vec<Option<Origin>>,
    pub tail: BTreeMap<Tag, Origin>,
}

impl Origins {
    pub(super) fn at(word: &Word, origin: Origin) -> Self {
        Self {
            prefix: vec![Some(origin); word.prefix.len()],
            tail: word.tail.iter().cloned().map(|tag| (tag, origin)).collect(),
        }
    }

    pub(super) fn merge(&mut self, other: Self, tags: &[Tag], allowed: &BTreeSet<Tag>) {
        let violation = |origins: &[Option<Origin>]| {
            tags.iter()
                .zip(origins)
                .find_map(|(tag, origin)| outside(tag, allowed).then_some(*origin).flatten())
        };
        // Keep the prefix from one witness. The tail records independent
        // representatives because the abstract word no longer observes order.
        let key = |origin: Option<Origin>| (origin.is_none(), origin.map(Origin::order));
        if key(violation(&other.prefix)) < key(violation(&self.prefix))
            || (key(violation(&other.prefix)) == key(violation(&self.prefix))
                && other
                    .prefix
                    .iter()
                    .copied()
                    .map(key)
                    .cmp(self.prefix.iter().copied().map(key))
                    .is_lt())
        {
            self.prefix = other.prefix;
        }
        for (tag, origin) in other.tail {
            self.insert_tail(tag, origin);
        }
    }

    fn insert_tail(&mut self, tag: Tag, origin: Origin) {
        self.tail
            .entry(tag)
            .and_modify(|current| {
                if origin.order() < current.order() {
                    *current = origin;
                }
            })
            .or_insert(origin);
    }

    pub(super) fn remap_tail(&self, map: impl Fn(&Tag) -> Tag) -> Self {
        let mut origins = Self {
            prefix: self.prefix.clone(),
            tail: BTreeMap::new(),
        };
        for (tag, &origin) in &self.tail {
            origins.insert_tail(map(tag), origin);
        }
        origins
    }

    pub(super) fn concatenate(
        left: &Word,
        right: &Word,
        a: Option<&Self>,
        b: Option<&Self>,
        available: usize,
    ) -> Self {
        let mut joined = Self {
            prefix: a.map_or_else(|| vec![None; left.prefix.len()], |a| a.prefix.clone()),
            tail: a.map_or_else(BTreeMap::new, |a| a.tail.clone()),
        };
        for (index, tag) in right.prefix.iter().enumerate() {
            let origin = b.and_then(|b| b.prefix[index]);
            if index < available {
                joined.prefix.push(origin);
            } else if let Some(origin) = origin {
                joined.insert_tail(tag.clone(), origin);
            }
        }
        if let Some(b) = b {
            for (tag, &origin) in &b.tail {
                joined.insert_tail(tag.clone(), origin);
            }
        }
        joined
    }
}
