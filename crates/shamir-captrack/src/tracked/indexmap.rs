use std::hash::Hash;

use indexmap::IndexMap;

use crate::{registry, ShamirHasher};

/// A `IndexMap<K, V, ShamirHasher>` wrapper — insertion-ordered map with
/// `FxHasher`.  This is the workspace `TMap` equivalent with capacity
/// telemetry.
pub struct TrackedIndexMap<K, V> {
    inner: IndexMap<K, V, ShamirHasher>,
    name: &'static str,
}

impl<K: Eq + Hash, V> TrackedIndexMap<K, V> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: IndexMap::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<K, V> std::ops::Deref for TrackedIndexMap<K, V> {
    type Target = IndexMap<K, V, ShamirHasher>;
    fn deref(&self) -> &IndexMap<K, V, ShamirHasher> {
        &self.inner
    }
}

impl<K, V> std::ops::DerefMut for TrackedIndexMap<K, V> {
    fn deref_mut(&mut self) -> &mut IndexMap<K, V, ShamirHasher> {
        &mut self.inner
    }
}

impl<K, V> Drop for TrackedIndexMap<K, V> {
    fn drop(&mut self) {
        registry::record_peak(self.name, self.inner.capacity());
    }
}

impl<K: Eq + Hash, V> IntoIterator for TrackedIndexMap<K, V> {
    type Item = (K, V);
    type IntoIter = indexmap::map::IntoIter<K, V>;

    fn into_iter(mut self) -> Self::IntoIter {
        registry::record_peak(self.name, self.inner.capacity());
        // IndexMap doesn't implement Default for all K,V — but it does when
        // K: Hash + Eq.  Use with_hasher(ShamirHasher::default()) to build
        // the replacement.
        let inner = std::mem::replace(
            &mut self.inner,
            IndexMap::with_hasher(ShamirHasher::default()),
        );
        std::mem::forget(self);
        inner.into_iter()
    }
}
