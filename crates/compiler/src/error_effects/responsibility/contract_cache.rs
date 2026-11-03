//! Reuse one fixed-point result only while every contract it read is unchanged.
//! The function universe is fixed for this validate invocation. Reads through
//! nested Engines and callable/receiver queries share the same dependency set;
//! no heap state, abstract facts or results survive into another compilation.
use super::Contract;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Index;

struct Entry {
    value: Contract,
    version: u64,
}

pub(super) struct Contracts {
    entries: BTreeMap<String, Entry>,
    dependencies: BTreeMap<String, BTreeMap<String, Option<u64>>>,
    reads: RefCell<BTreeMap<String, Option<u64>>>,
}

impl Contracts {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        Self {
            entries: names
                .into_iter()
                .map(|name| {
                    (
                        name,
                        Entry {
                            value: Contract::default(),
                            version: 0,
                        },
                    )
                })
                .collect(),
            dependencies: BTreeMap::new(),
            reads: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn begin(&self) {
        self.reads.borrow_mut().clear();
    }

    pub fn reusable(&self, name: &str) -> bool {
        self.dependencies.get(name).is_some_and(|dependencies| {
            dependencies.iter().all(|(dependency, version)| {
                self.entries.get(dependency).map(|entry| entry.version) == *version
            })
        })
    }

    pub fn get(&self, name: &str) -> Option<&Contract> {
        let entry = self.entries.get(name);
        let mut reads = self.reads.borrow_mut();
        if !reads.contains_key(name) {
            reads.insert(name.to_owned(), entry.map(|entry| entry.version));
        }
        entry.map(|entry| &entry.value)
    }

    // Enumeration observes only the immutable function universe. Reading a
    // value found through this iterator still goes through get or Index.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    pub fn update(&mut self, name: &str, next: Contract) -> bool {
        // Capture versions before publishing the new result. In particular,
        // a changed recursive self-read must invalidate its next iteration.
        self.dependencies
            .insert(name.to_owned(), std::mem::take(self.reads.get_mut()));
        let entry = self.entries.get_mut(name).expect("known contract");
        if entry.value == next {
            return false;
        }
        entry.value = next;
        entry.version = entry
            .version
            .checked_add(1)
            .expect("too many contract revisions");
        true
    }

    // Used after the fixed point, solely to emit diagnostics in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Contract)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name, &entry.value))
    }
}

impl Index<&String> for Contracts {
    type Output = Contract;
    fn index(&self, name: &String) -> &Self::Output {
        self.get(name).expect("known contract")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contracts() -> Contracts {
        Contracts::new(["a", "b", "c"].map(str::to_owned))
    }

    #[test]
    fn only_actual_dependencies_invalidate_a_result() {
        let mut cache = contracts();
        assert!(!cache.reusable("a"));
        cache.begin();
        assert!(!cache.get("b").unwrap().mutates);
        assert!(!cache.update("a", Contract::default()));
        assert!(cache.reusable("a"));
        cache.begin();
        assert!(cache.update(
            "c",
            Contract {
                mutates: true,
                ..Contract::default()
            }
        ));
        assert!(cache.reusable("a"));
        cache.begin();
        assert!(cache.update(
            "b",
            Contract {
                mutates: true,
                ..Contract::default()
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
        cache.update("a", Contract::default());
        cache.begin();
        let _ = cache.get("c");
        cache.update("a", Contract::default());
        cache.begin();
        cache.update(
            "b",
            Contract {
                mutates_async: true,
                ..Contract::default()
            },
        );
        assert!(cache.reusable("a"));
        cache.begin();
        cache.update(
            "c",
            Contract {
                disrupts_observers: true,
                ..Contract::default()
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
        cache.update("a", Contract::default());
        assert!(cache.reusable("a"));
        cache.begin();
        cache.update(
            "b",
            Contract {
                mutates: true,
                ..Contract::default()
            },
        );
        assert!(cache.reusable("a"));
    }
}
