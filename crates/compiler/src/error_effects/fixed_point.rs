//! Reuse an analysis result while every summary it read is unchanged. The
//! function universe and analysis inputs are fixed for each invocation.
use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::ops::Index;
use std::sync::Arc;

use crate::function_cache::{self as memo, Fingerprint, Read, SharedSummary};

#[derive(Clone)]
struct Entry<T> {
    value: Arc<T>,
    version: u64,
    fingerprint: OnceCell<Option<Fingerprint>>,
}

pub(super) struct FixedPoint<T> {
    entries: BTreeMap<String, Entry<T>>,
    dependencies: BTreeMap<String, BTreeMap<String, Option<u64>>>,
    reads: RefCell<Option<BTreeMap<String, Option<u64>>>>,
    observation: Option<fn(&str) -> Read>,
    absent: OnceCell<Option<Fingerprint>>,
}

impl<T: Default + PartialEq> FixedPoint<T> {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            entries: names
                .into_iter()
                .map(|name| {
                    (
                        name,
                        Entry {
                            value: Arc::new(T::default()),
                            version: 0,
                            fingerprint: OnceCell::new(),
                        },
                    )
                })
                .collect(),
            dependencies: BTreeMap::new(),
            reads: RefCell::new(None),
            observation: None,
            absent: OnceCell::new(),
        }
    }

    /// Share the converged values with another validation pass while keeping
    /// each pass's observation state and lazy fingerprints thread-local.
    pub fn snapshot(&self) -> Self
    where
        T: Clone,
    {
        Self {
            entries: self.entries.clone(),
            dependencies: BTreeMap::new(),
            reads: RefCell::new(None),
            observation: self.observation,
            absent: self.absent.clone(),
        }
    }

    pub fn observe(&mut self, observation: fn(&str) -> Read) {
        self.observation = Some(observation);
    }

    pub fn fingerprint(&self, name: &str) -> Option<Fingerprint>
    where
        T: serde::Serialize,
    {
        match self.entries.get(name) {
            Some(entry) => *entry
                .fingerprint
                .get_or_init(|| memo::fingerprint(Some(&entry.value))),
            None => *self
                .absent
                .get_or_init(|| memo::fingerprint(Option::<T>::None)),
        }
    }

    pub fn begin(&self) {
        *self.reads.borrow_mut() = Some(BTreeMap::new());
    }

    pub fn reusable(&self, name: &str) -> bool {
        self.dependencies.get(name).is_some_and(|dependencies| {
            dependencies.iter().all(|(dependency, version)| {
                self.entries.get(dependency).map(|entry| entry.version) == *version
            })
        })
    }

    pub fn get(&self, name: &str) -> Option<&T> {
        if let Some(observation) = self.observation {
            memo::read(|| observation(name));
        }
        let entry = self.entries.get(name);
        if let Some(reads) = self.reads.borrow_mut().as_mut()
            && !reads.contains_key(name)
        {
            reads.insert(name.to_owned(), entry.map(|entry| entry.version));
        }
        entry.map(|entry| entry.value.as_ref())
    }

    // Enumeration observes only the immutable function universe. Reading a
    // value found through this iterator still goes through get or Index.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn update(&mut self, name: &str, next: T) -> bool {
        self.publish(name, Arc::new(next), None)
    }

    pub fn update_shared(&mut self, name: &str, next: SharedSummary<T>) -> bool {
        self.publish(name, next.value, next.digest)
    }

    fn publish(&mut self, name: &str, next: Arc<T>, digest: Option<Fingerprint>) -> bool {
        // Capture versions before publishing the new result. In particular,
        // a changed recursive self-read must invalidate its next iteration.
        self.dependencies.insert(
            name.to_owned(),
            self.reads.get_mut().take().expect("active analysis"),
        );
        let entry = self.entries.get_mut(name).expect("known summary");
        if entry.value == next {
            return false;
        }
        entry.value = next;
        entry.fingerprint.take();
        if let Some(digest) = digest {
            let _ = entry.fingerprint.set(Some(digest));
        }
        entry.version = entry
            .version
            .checked_add(1)
            .expect("too many summary revisions");
        true
    }

    // Used after the fixed point, solely to emit diagnostics in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &T)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name, entry.value.as_ref()))
    }

    pub fn into_values(self) -> BTreeMap<String, T>
    where
        T: Clone,
    {
        self.entries
            .into_iter()
            .map(|(name, entry)| (name, Arc::unwrap_or_clone(entry.value)))
            .collect()
    }
}

impl<T: Default + PartialEq> Index<&String> for FixedPoint<T> {
    type Output = T;
    fn index(&self, name: &String) -> &Self::Output {
        self.get(name).expect("known summary")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default, PartialEq)]
    struct Summary {
        mutates: bool,
        mutates_async: bool,
        disrupts_observers: bool,
    }

    fn contracts() -> FixedPoint<Summary> {
        FixedPoint::new(["a", "b", "c"].map(str::to_owned))
    }

    #[test]
    fn only_actual_dependencies_invalidate_a_result() {
        let mut cache = contracts();
        assert!(!cache.reusable("a"));
        cache.begin();
        assert!(!cache.get("b").unwrap().mutates);
        assert!(!cache.update("a", Summary::default()));
        assert!(cache.reusable("a"));
        cache.begin();
        assert!(cache.update(
            "c",
            Summary {
                mutates: true,
                ..Summary::default()
            }
        ));
        assert!(cache.reusable("a"));
        cache.begin();
        assert!(cache.update(
            "b",
            Summary {
                mutates: true,
                ..Summary::default()
            }
        ));
        assert!(!cache.reusable("a"));
    }

    #[test]
    fn changed_self_reads_require_another_iteration() {
        let mut cache = contracts();
        cache.begin();
        let mut next = cache[&"a".to_owned()].clone();
        next.mutates = true;
        assert!(cache.update("a", next));
        assert!(!cache.reusable("a"));
        cache.begin();
        let next = cache.get("a").unwrap().clone();
        assert!(!cache.update("a", next));
        assert!(cache.reusable("a"));
    }

    #[test]
    fn conditional_reads_replace_the_previous_dependency_set() {
        let mut cache = contracts();
        cache.begin();
        let _ = cache.get("b");
        cache.update("a", Summary::default());
        cache.begin();
        let _ = cache.get("c");
        cache.update("a", Summary::default());
        cache.begin();
        cache.update(
            "b",
            Summary {
                mutates_async: true,
                ..Summary::default()
            },
        );
        assert!(cache.reusable("a"));
        cache.begin();
        cache.update(
            "c",
            Summary {
                disrupts_observers: true,
                ..Summary::default()
            },
        );
        assert!(!cache.reusable("a"));
    }

    #[test]
    fn missing_reads_and_universe_iteration_remain_stable() {
        let mut cache = contracts();
        cache.begin();
        assert!(cache.get("native").is_none());
        assert_eq!(
            cache.keys().map(String::as_str).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        cache.update("a", Summary::default());
        assert!(cache.reusable("a"));
        cache.begin();
        cache.update(
            "b",
            Summary {
                mutates: true,
                ..Summary::default()
            },
        );
        assert!(cache.reusable("a"));
    }
}
