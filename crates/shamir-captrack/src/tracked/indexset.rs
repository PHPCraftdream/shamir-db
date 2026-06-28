use std::hash::Hash;

use indexmap::IndexSet;

use crate::{registry, ShamirHasher};

/// A `IndexSet<T, ShamirHasher>` wrapper — insertion-ordered set with
/// `FxHasher`.  This is the workspace `TSet` equivalent with capacity
/// telemetry.
pub struct TrackedIndexSet<T> {
    inner: IndexSet<T, ShamirHasher>,
    name: &'static str,
}

impl<T: Eq + Hash> TrackedIndexSet<T> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: IndexSet::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<T> std::ops::Deref for TrackedIndexSet<T> {
    type Target = IndexSet<T, ShamirHasher>;
    fn deref(&self) -> &IndexSet<T, ShamirHasher> {
        &self.inner
    }
}

impl<T> std::ops::DerefMut for TrackedIndexSet<T> {
    fn deref_mut(&mut self) -> &mut IndexSet<T, ShamirHasher> {
        &mut self.inner
    }
}

impl<T> Drop for TrackedIndexSet<T> {
    fn drop(&mut self) {
        registry::record_peak(self.name, self.inner.capacity());
    }
}

impl<T: Eq + Hash> IntoIterator for TrackedIndexSet<T> {
    type Item = T;
    type IntoIter = indexmap::set::IntoIter<T>;

    fn into_iter(mut self) -> Self::IntoIter {
        registry::record_peak(self.name, self.inner.capacity());
        let inner = std::mem::replace(
            &mut self.inner,
            IndexSet::with_hasher(ShamirHasher::default()),
        );
        std::mem::forget(self);
        inner.into_iter()
    }
}
