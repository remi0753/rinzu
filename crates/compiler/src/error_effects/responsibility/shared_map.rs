//! Copy-on-write analysis state. Sharing changes storage, never the abstract
//! facts: equality still compares values, and every mutable access detaches.
use std::borrow::Borrow;
use std::cell::OnceCell;
use std::collections::{BTreeMap, btree_map};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Clone, Debug)]
struct Entry<V> {
    value: V,
    hash: OnceCell<u64>,
}
#[derive(Clone, Debug)]
struct Shared<V>(Rc<Entry<V>>);
impl<V: PartialEq> PartialEq for Shared<V> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0) || self.0.value == other.0.value
    }
}
impl<V: Eq> Eq for Shared<V> {}
impl<V: Hash> Hash for Shared<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // This is only an index key. Hash collisions always reach value Eq.
        state.write_u64(*self.0.hash.get_or_init(|| {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            self.0.value.hash(&mut hash);
            hash.finish()
        }));
    }
}
impl<V> Shared<V> {
    fn new(value: V) -> Self {
        Self(Rc::new(Entry {
            value,
            hash: OnceCell::new(),
        }))
    }
}
impl<V: Clone> Shared<V> {
    fn make_mut(&mut self) -> &mut V {
        let entry = Rc::make_mut(&mut self.0);
        entry.hash.take();
        &mut entry.value
    }
    fn into_value(self) -> V {
        Rc::unwrap_or_clone(self.0).value
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(super) struct SharedMap<K: Ord, V>(Rc<BTreeMap<K, Shared<V>>>);
impl<K: Ord, V> Default for SharedMap<K, V> {
    fn default() -> Self {
        Self(Rc::new(BTreeMap::new()))
    }
}
impl<K: Ord, V> SharedMap<K, V> {
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.0.get(key).map(|value| &value.0.value)
    }
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.0.contains_key(key)
    }
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.0.keys()
    }
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.values().map(|value| &value.0.value)
    }
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter(self.0.iter())
    }
}
impl<K: Ord + Clone, V: Clone> SharedMap<K, V> {
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        Rc::make_mut(&mut self.0).get_mut(key).map(Shared::make_mut)
    }
    pub fn insert(&mut self, key: K, value: V) {
        Rc::make_mut(&mut self.0).insert(key, Shared::new(value));
    }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        Rc::make_mut(&mut self.0)
            .remove(key)
            .map(Shared::into_value)
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        Rc::make_mut(&mut self.0).values_mut().map(Shared::make_mut)
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        Rc::make_mut(&mut self.0)
            .iter_mut()
            .map(|(key, value)| (key, value.make_mut()))
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        // Scope exit usually removes no entries. Preserve the shared map in
        // that case, instead of cloning its tree just to inspect its keys.
        let removed: std::collections::BTreeSet<_> = self
            .iter()
            .filter(|(key, value)| !keep(key, value))
            .map(|(key, _)| key.clone())
            .collect();
        if removed.is_empty() {
            return;
        }
        Rc::make_mut(&mut self.0).retain(|key, _| !removed.contains(key));
    }
    pub fn rename_keys(self, mut rename: impl FnMut(K) -> K) -> Self {
        Self(Rc::new(
            Rc::unwrap_or_clone(self.0)
                .into_iter()
                .map(|(key, value)| (rename(key), value))
                .collect(),
        ))
    }
    pub fn filter_map_keys(self, mut rename: impl FnMut(K) -> Option<K>) -> Self {
        Self(Rc::new(
            Rc::unwrap_or_clone(self.0)
                .into_iter()
                .filter_map(|(key, value)| rename(key).map(|key| (key, value)))
                .collect(),
        ))
    }
}
impl<K: Ord, V> FromIterator<(K, V)> for SharedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(Rc::new(
            iter.into_iter()
                .map(|(key, value)| (key, Shared::new(value)))
                .collect(),
        ))
    }
}
pub(super) struct Iter<'a, K, V>(btree_map::Iter<'a, K, Shared<V>>);
impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(key, value)| (key, &value.0.value))
    }
}
impl<'a, K: Ord, V> IntoIterator for &'a SharedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hash(value: &impl Hash) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        value.hash(&mut hash);
        hash.finish()
    }
    #[test]
    fn detach_preserves_aliases_and_invalidates_cached_hashes() {
        let original: SharedMap<_, _> = [("value".to_owned(), vec![1, 2])].into_iter().collect();
        let before = hash(&original);
        let mut branch = original.clone();
        branch.get_mut("value").unwrap().push(3);
        assert_eq!(original.get("value"), Some(&vec![1, 2]));
        assert_ne!(hash(&branch), before);
        let rebuilt: SharedMap<_, _> = [("value".to_owned(), vec![1, 2, 3])].into_iter().collect();
        assert_eq!(branch, rebuilt);
        assert_eq!(hash(&branch), hash(&rebuilt));
        let unchanged = branch.clone();
        for value in branch.values_mut() {
            value.push(4);
        }
        assert_eq!(unchanged, rebuilt);
        assert_ne!(hash(&branch), hash(&rebuilt));
        let renamed = unchanged.rename_keys(|key| format!("caller:{key}"));
        assert_eq!(renamed.get("caller:value"), Some(&vec![1, 2, 3]));
        let restored =
            renamed.filter_map_keys(|key| key.strip_prefix("caller:").map(str::to_owned));
        assert_eq!(restored, rebuilt);
    }
}
