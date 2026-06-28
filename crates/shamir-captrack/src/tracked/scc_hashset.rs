use std::hash::Hash;

use crate::{registry, ShamirHasher};

/// A `scc::HashSet<T, ShamirHasher>` wrapper that records creation count and
/// peak occupancy.
///
/// `scc::HashSet::len()` is O(N) — only called on Drop (telemetry path).
pub struct TrackedSccHashSet<T>
where
    T: Eq + Hash + 'static,
{
    inner: scc::HashSet<T, ShamirHasher>,
    name: &'static str,
}

impl<T: Eq + Hash + 'static> TrackedSccHashSet<T> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: scc::HashSet::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<T: Eq + Hash + 'static> std::ops::Deref for TrackedSccHashSet<T> {
    type Target = scc::HashSet<T, ShamirHasher>;
    fn deref(&self) -> &scc::HashSet<T, ShamirHasher> {
        &self.inner
    }
}

impl<T: Eq + Hash + 'static> std::ops::DerefMut for TrackedSccHashSet<T> {
    fn deref_mut(&mut self) -> &mut scc::HashSet<T, ShamirHasher> {
        &mut self.inner
    }
}

impl<T: Eq + Hash + 'static> Drop for TrackedSccHashSet<T> {
    fn drop(&mut self) {
        // O(N) ack: telemetry only — scc::HashSet::len() is a full traversal.
        #[allow(clippy::disallowed_methods)]
        let peak = self.inner.len();
        registry::record_peak(self.name, peak);
    }
}
