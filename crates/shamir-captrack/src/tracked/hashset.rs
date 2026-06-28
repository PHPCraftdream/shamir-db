// TrackedHashSet wraps std::HashSet which is banned by the workspace
// disallowed_types lint.  This file is the ONE sanctioned allow-site for the
// set wrapper — identical to the pattern in shamir-collections.
#![allow(clippy::disallowed_types)]

use std::collections::HashSet;
use std::hash::Hash;

use crate::{registry, ShamirHasher};

/// A `HashSet<T, ShamirHasher>` wrapper that records creation count and peak
/// capacity.
pub struct TrackedHashSet<T> {
    inner: HashSet<T, ShamirHasher>,
    name: &'static str,
}

impl<T: Eq + Hash> TrackedHashSet<T> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: HashSet::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<T> std::ops::Deref for TrackedHashSet<T> {
    type Target = HashSet<T, ShamirHasher>;
    fn deref(&self) -> &HashSet<T, ShamirHasher> {
        &self.inner
    }
}

impl<T> std::ops::DerefMut for TrackedHashSet<T> {
    fn deref_mut(&mut self) -> &mut HashSet<T, ShamirHasher> {
        &mut self.inner
    }
}

impl<T> Drop for TrackedHashSet<T> {
    fn drop(&mut self) {
        registry::record_peak(self.name, self.inner.capacity());
    }
}

impl<T: Eq + Hash> IntoIterator for TrackedHashSet<T> {
    type Item = T;
    type IntoIter = std::collections::hash_set::IntoIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        registry::record_peak(self.name, self.inner.capacity());
        let inner = std::mem::take(&mut self.inner);
        std::mem::forget(self);
        inner.into_iter()
    }
}
