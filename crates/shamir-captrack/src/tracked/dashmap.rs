use std::hash::Hash;

use dashmap::DashMap;

use crate::{registry, ShamirHasher};

/// A `DashMap<K, V, ShamirHasher>` wrapper that records creation count and
/// peak occupancy.
///
/// `DashMap` does not expose a `capacity()` method, so peak is measured via
/// `len()` on Drop.
///
/// # Telemetry note
/// `DashMap::len()` is O(N) — it iterates all shards.  This call is
/// intentionally limited to the Drop path (telemetry only, not a hot path).
pub struct TrackedDashMap<K, V>
where
    K: Eq + Hash,
{
    inner: DashMap<K, V, ShamirHasher>,
    name: &'static str,
}

impl<K: Eq + Hash, V> TrackedDashMap<K, V> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: DashMap::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<K: Eq + Hash, V> std::ops::Deref for TrackedDashMap<K, V> {
    type Target = DashMap<K, V, ShamirHasher>;
    fn deref(&self) -> &DashMap<K, V, ShamirHasher> {
        &self.inner
    }
}

impl<K: Eq + Hash, V> std::ops::DerefMut for TrackedDashMap<K, V> {
    fn deref_mut(&mut self) -> &mut DashMap<K, V, ShamirHasher> {
        &mut self.inner
    }
}

impl<K: Eq + Hash, V> Drop for TrackedDashMap<K, V> {
    fn drop(&mut self) {
        // O(N) ack: telemetry only — not a hot path.
        #[allow(clippy::disallowed_methods)]
        let peak = self.inner.len();
        registry::record_peak(self.name, peak);
    }
}
