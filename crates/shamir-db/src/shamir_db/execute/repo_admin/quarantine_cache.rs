//! Lock-free snapshot cache of `resync_required` quarantine coverage
//! (S5c-X PR8b-2). An immutable [`QuarantineSnapshot`] is published through
//! an [`arc_swap::ArcSwap`]: reads are two borrow-only hash lookups keyed by
//! exact `(db, repo)` — no global sentinel, no wildcard arms, no map locks,
//! and zero allocation unless a quarantine hit clones the reason.
//!
//! Reliability contract: there is NO global sentinel anymore. A catalogue
//! validation failure fails initialization (typed error at open) or, at
//! runtime, retains the last safe snapshot — `install` is simply not called
//! on error, so protection is never cleared.

use arc_swap::ArcSwap;
use shamir_collections::{TFxMap, TSet};
use std::sync::Arc;

type QuarantineEntries = TFxMap<String, QuarantineRepos>;
type QuarantineRepos = TFxMap<String, Arc<str>>;

/// Immutable snapshot: two levels keyed by db then repo, plus the set of
/// prepublished (not yet durably confirmed) keys.
#[derive(Default)]
pub(crate) struct QuarantineSnapshot {
    pub(crate) entries: QuarantineEntries,
    pending: TSet<(String, String)>,
}

pub(crate) struct ResyncQuarantineCache {
    snapshot: ArcSwap<QuarantineSnapshot>,
}

impl ResyncQuarantineCache {
    pub(crate) fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(QuarantineSnapshot::default()),
        }
    }
    /// One `load()` of the current snapshot. Callers resolve their own
    /// lookups against the returned guard so a lookup is exactly ONE atomic
    /// load (no `is_empty` fast-path load before it — the healthy path is a
    /// miss on an empty map, still allocation-free).
    pub(crate) fn snapshot(&self) -> arc_swap::Guard<Arc<QuarantineSnapshot>> {
        self.snapshot.load()
    }
    /// Exact `(db, repo)` lookup only — no wildcard or global fallback.
    pub(crate) fn reason_for(&self, db: &str, repo: &str) -> Option<String> {
        let snapshot = self.snapshot.load();
        snapshot
            .entries
            .get(db)?
            .get(repo)
            .map(|reason| reason.to_string())
    }
    /// Add pre-persistence quarantine keys, retaining them if persistence
    /// fails. Existing keys keep their current reason (old vacant-only
    /// semantics); newly added keys are recorded as pending. Called under
    /// `replication_control_lock`, before persistence.
    pub(crate) fn publish(&self, entries: impl IntoIterator<Item = ((String, String), String)>) {
        let current = self.snapshot.load_full();
        let mut next = QuarantineSnapshot {
            entries: current.entries.clone(),
            pending: current.pending.clone(),
        };
        for ((db, repo), reason) in entries {
            let repos = next.entries.entry(db.clone()).or_default();
            if !repos.contains_key(repo.as_str()) {
                repos.insert(repo.clone(), Arc::from(reason));
                next.pending.insert((db, repo));
            }
        }
        self.snapshot.store(Arc::new(next));
    }
    /// Replace cache contents from a successful persistence snapshot. The
    /// durable rows are the single source of truth at refresh time: a key
    /// the refreshed rows no longer derive is dropped, even if it was
    /// prepublished by [`publish`]. The pending set protects only the
    /// window BEFORE refresh — reads and the drop/rename guards see the
    /// prepublished keys via `entries`/`reason_for` until the next
    /// successful refresh confirms or clears them. On persist/flush
    /// FAILURE refresh is never reached, so the last safe snapshot
    /// (including the prepublished keys) stays — protection never
    /// regresses.
    pub(crate) fn install(&self, entries: impl IntoIterator<Item = ((String, String), String)>) {
        let current = self.snapshot.load_full();
        let mut fresh: TSet<(String, String)> = TSet::default();
        let mut next_entries: QuarantineEntries = TFxMap::default();
        for ((db, repo), reason) in entries {
            fresh.insert((db.clone(), repo.clone()));
            next_entries
                .entry(db)
                .or_default()
                .insert(repo, Arc::from(reason));
        }
        let mut pending = current.pending.clone();
        for key in fresh.iter() {
            // Deprecated `IndexSet::remove` is not used: order in the
            // pending set is irrelevant, so `swap_remove` is equivalent.
            pending.swap_remove(key);
        }
        // Prepublished keys the refreshed rows no longer derive are
        // dropped with the pending set: the durable rows are the single
        // source of truth at refresh time.
        self.snapshot.store(Arc::new(QuarantineSnapshot {
            entries: next_entries,
            pending,
        }));
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
