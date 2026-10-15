//! Cache published before persistent resync transitions.
use dashmap::DashMap;
use shamir_collections::{THasher, TSet};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct ResyncQuarantineCache {
    entries: DashMap<(String, String), String, THasher>,
    pending: DashMap<(String, String), (), THasher>,
    count: AtomicUsize,
}
impl ResyncQuarantineCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: DashMap::with_hasher(THasher::default()),
            pending: DashMap::with_hasher(THasher::default()),
            count: AtomicUsize::new(0),
        }
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.count.load(Ordering::Acquire) == 0
    }
    pub(crate) fn reason_for(&self, db: &str, repo: &str) -> Option<String> {
        if let Some(reason) = self.entries.get(&(db.to_string(), repo.to_string())) {
            return Some(reason.value().clone());
        }
        self.entries
            .get(&(db.to_string(), "*".to_string()))
            .map(|reason| reason.value().clone())
    }
    /// Add pre-persistence quarantine keys, retaining them if persistence fails.
    pub(crate) fn publish(&self, entries: impl IntoIterator<Item = ((String, String), String)>) {
        for (key, reason) in entries {
            if let dashmap::mapref::entry::Entry::Vacant(entry) = self.entries.entry(key.clone()) {
                entry.insert(reason);
                self.pending.insert(key, ());
                self.count.fetch_add(1, Ordering::Release);
            }
        }
    }
    /// Replace cache contents from a successful persistence snapshot, keeping
    /// prepublished keys until that snapshot confirms their durable state.
    pub(crate) fn install(&self, entries: impl IntoIterator<Item = ((String, String), String)>) {
        let mut fresh: TSet<(String, String)> = TSet::default();
        for (key, reason) in entries {
            self.entries.insert(key.clone(), reason);
            fresh.insert(key);
        }
        self.pending.retain(|key, _| !fresh.contains(key));
        self.entries
            .retain(|key, _| fresh.contains(key) || self.pending.contains_key(key));
        self.count.store(self.entries.len(), Ordering::Release);
    }
}
impl Default for ResyncQuarantineCache {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
tokio::task_local! {
    pub(crate) static SKIP_READ_QUARANTINE_FOR_TEST: bool;
}

#[inline]
pub(crate) fn skip_read_quarantine_for_test() -> bool {
    #[cfg(test)]
    let skip = SKIP_READ_QUARANTINE_FOR_TEST
        .try_with(|skip| *skip)
        .unwrap_or(false);
    #[cfg(not(test))]
    let skip = false;
    skip
}
