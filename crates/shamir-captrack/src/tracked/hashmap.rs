// TrackedHashMap wraps std::HashMap which is banned by the workspace
// disallowed_types lint.  This file is the ONE sanctioned allow-site for the
// wrapper type itself — identical to the pattern in shamir-collections.
#![allow(clippy::disallowed_types)]

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

use crate::registry;

/// A `HashMap<K, V, S>` wrapper that records creation count and peak capacity.
///
/// The hasher type `S` is generic; for the `FxHasher`-specialised variant see
/// `TrackedFxHashMap`.
pub struct TrackedHashMap<K, V, S> {
    inner: HashMap<K, V, S>,
    name: &'static str,
}

impl<K: Eq + Hash, V, S: BuildHasher + Default> TrackedHashMap<K, V, S> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: HashMap::with_capacity_and_hasher(cap, S::default()),
            name,
        }
    }
}

impl<K, V, S> std::ops::Deref for TrackedHashMap<K, V, S> {
    type Target = HashMap<K, V, S>;
    fn deref(&self) -> &HashMap<K, V, S> {
        &self.inner
    }
}

impl<K, V, S> std::ops::DerefMut for TrackedHashMap<K, V, S> {
    fn deref_mut(&mut self) -> &mut HashMap<K, V, S> {
        &mut self.inner
    }
}

impl<K, V, S> Drop for TrackedHashMap<K, V, S> {
    fn drop(&mut self) {
        registry::record_peak(self.name, self.inner.capacity());
    }
}

impl<K: Eq + Hash, V, S: BuildHasher + Default> IntoIterator for TrackedHashMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = std::collections::hash_map::IntoIter<K, V>;

    fn into_iter(mut self) -> Self::IntoIter {
        registry::record_peak(self.name, self.inner.capacity());
        let inner = std::mem::replace(&mut self.inner, HashMap::with_hasher(S::default()));
        std::mem::forget(self);
        inner.into_iter()
    }
}
