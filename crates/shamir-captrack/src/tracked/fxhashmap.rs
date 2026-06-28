// TrackedFxHashMap wraps std::HashMap which is banned by the workspace
// disallowed_types lint.  This file is the ONE sanctioned allow-site for the
// Fx-hasher variant — identical to the pattern in shamir-collections.
#![allow(clippy::disallowed_types)]

use std::collections::HashMap;
use std::hash::Hash;

use crate::{registry, ShamirHasher};

/// A `HashMap<K, V, ShamirHasher>` wrapper — `std::HashMap` with `FxHasher`
/// as the hasher.  This is the workspace-default for order-agnostic maps.
pub struct TrackedFxHashMap<K, V> {
    inner: HashMap<K, V, ShamirHasher>,
    name: &'static str,
}

impl<K: Eq + Hash, V> TrackedFxHashMap<K, V> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: HashMap::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<K, V> std::ops::Deref for TrackedFxHashMap<K, V> {
    type Target = HashMap<K, V, ShamirHasher>;
    fn deref(&self) -> &HashMap<K, V, ShamirHasher> {
        &self.inner
    }
}

impl<K, V> std::ops::DerefMut for TrackedFxHashMap<K, V> {
    fn deref_mut(&mut self) -> &mut HashMap<K, V, ShamirHasher> {
        &mut self.inner
    }
}

impl<K, V> Drop for TrackedFxHashMap<K, V> {
    fn drop(&mut self) {
        registry::record_peak(self.name, self.inner.capacity());
    }
}

impl<K: Eq + Hash, V> IntoIterator for TrackedFxHashMap<K, V> {
    type Item = (K, V);
    type IntoIter = std::collections::hash_map::IntoIter<K, V>;

    fn into_iter(mut self) -> Self::IntoIter {
        registry::record_peak(self.name, self.inner.capacity());
        let inner = std::mem::take(&mut self.inner);
        std::mem::forget(self);
        inner.into_iter()
    }
}
