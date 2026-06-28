use std::hash::Hash;

use crate::{registry, ShamirHasher};

/// A `scc::HashMap<K, V, ShamirHasher>` wrapper that records creation count
/// and peak occupancy.
///
/// `scc::HashMap::len()` is O(N) — it is only called on the Drop path
/// (telemetry only, never on a hot path).
pub struct TrackedSccHashMap<K, V>
where
    K: Eq + Hash + 'static,
    V: 'static,
{
    inner: scc::HashMap<K, V, ShamirHasher>,
    name: &'static str,
}

impl<K: Eq + Hash + 'static, V: 'static> TrackedSccHashMap<K, V> {
    pub fn with_capacity_named(cap: usize, name: &'static str) -> Self {
        registry::record_creation(name);
        Self {
            inner: scc::HashMap::with_capacity_and_hasher(cap, ShamirHasher::default()),
            name,
        }
    }
}

impl<K: Eq + Hash + 'static, V: 'static> std::ops::Deref for TrackedSccHashMap<K, V> {
    type Target = scc::HashMap<K, V, ShamirHasher>;
    fn deref(&self) -> &scc::HashMap<K, V, ShamirHasher> {
        &self.inner
    }
}

impl<K: Eq + Hash + 'static, V: 'static> std::ops::DerefMut for TrackedSccHashMap<K, V> {
    fn deref_mut(&mut self) -> &mut scc::HashMap<K, V, ShamirHasher> {
        &mut self.inner
    }
}

impl<K: Eq + Hash + 'static, V: 'static> Drop for TrackedSccHashMap<K, V> {
    fn drop(&mut self) {
        // O(N) ack: telemetry only — scc::HashMap::len() is a full traversal,
        // but this only runs on Drop, not on any hot path.
        #[allow(clippy::disallowed_methods)]
        let peak = self.inner.len();
        registry::record_peak(self.name, peak);
    }
}
