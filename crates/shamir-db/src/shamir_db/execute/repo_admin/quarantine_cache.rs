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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

type QuarantineEntries = TFxMap<String, QuarantineRepos>;
type QuarantineRepos = TFxMap<String, Arc<str>>;
/// Round 7 item 5: pending keys are OWNER-SCOPED — each `(db, repo)` key
/// maps to the set of `name:generation:token` claim strings holding it. A
/// rollback/confirm removes ONLY its own owner's claim, so an unrelated
/// subscription's successful Resume can never erase another mark's pending
/// protection (a bare-key rollback could). Entries with an EMPTY owner set
/// are removed eagerly — the set is only ever observed non-empty.
type PendingOwners = TFxMap<(String, String), TSet<String>>;

/// Immutable snapshot: two levels keyed by db then repo, plus the set of
/// prepublished (not yet durably confirmed) keys and the set of keys the
/// LAST successful refresh derived from the durable rows (`derived` —
/// round 6 item 5: `confirm_pending` needs it to tell a rows-backed key,
/// whose entry survives confirmation, from a pending-only key, whose entry
/// must go).
#[derive(Default)]
pub(crate) struct QuarantineSnapshot {
    pub(crate) entries: QuarantineEntries,
    pending: PendingOwners,
    derived: TSet<(String, String)>,
}

pub(crate) struct ResyncQuarantineCache {
    snapshot: ArcSwap<QuarantineSnapshot>,
    claim_token: AtomicU64,
}

impl ResyncQuarantineCache {
    pub(crate) fn new() -> Self {
        Self {
            snapshot: ArcSwap::from_pointee(QuarantineSnapshot::default()),
            claim_token: AtomicU64::new(0),
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
    /// Add pre-persistence quarantine keys, retaining them if persistence fails.
    /// Refresh-derived keys are preserved; only keys passed by this mark are published.
    /// Called under `replication_control_lock`, before persistence.
    pub(crate) fn publish(
        &self,
        entries: impl IntoIterator<Item = ((String, String), String)>,
        owner: &str,
    ) -> String {
        let token = self
            .claim_token
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_add(1))
            .expect("quarantine claim token exhausted");
        let owner = format!("{owner}:{token}");
        let current = self.snapshot.load_full();
        let mut next = QuarantineSnapshot {
            entries: current.entries.clone(),
            pending: current.pending.clone(),
            derived: current.derived.clone(),
        };
        for ((db, repo), reason) in entries {
            let key = (db.clone(), repo.clone());
            let repos = next.entries.entry(db.clone()).or_default();
            repos.entry(repo).or_insert_with(|| Arc::from(reason));
            let claims = next.pending.entry(key.clone()).or_default();
            claims.insert(owner.clone());
        }
        self.snapshot.store(Arc::new(next));
        owner
    }

    /// Remove only the exact claim (including generation/token identity).
    /// Returns true when the key's last claim was removed.
    fn remove_owner_claims(
        next: &mut QuarantineSnapshot,
        key: &(String, String),
        claim: &str,
    ) -> bool {
        let Some(claims) = next.pending.get_mut(key) else {
            return false;
        };
        let stale: Vec<String> = claims
            .iter()
            .filter(|stored| stored.as_str() == claim)
            .cloned()
            .collect();
        for claim in stale {
            claims.shift_remove(&claim);
        }
        if claims.is_empty() {
            next.pending.remove(key);
            return true;
        }
        false
    }
    pub(crate) fn owner_claims_for(&self, owner: &str, keys: &[(String, String)]) -> Vec<String> {
        let snapshot = self.snapshot.load();
        let mut claims = Vec::new();
        for key in keys {
            if let Some(owners) = snapshot.pending.get(key) {
                for claim in owners.iter().filter(|claim| {
                    claim
                        .rsplit_once(':')
                        .is_some_and(|(prefix, _)| prefix == owner)
                }) {
                    if !claims.contains(claim) {
                        claims.push(claim.clone());
                    }
                }
            }
        }
        claims
    }
    pub(crate) fn owner_claims_for_subscription(
        &self,
        name: &str,
        keys: &[(String, String)],
    ) -> Vec<String> {
        let snapshot = self.snapshot.load();
        let mut claims = Vec::new();
        for key in keys {
            if let Some(owners) = snapshot.pending.get(key) {
                for claim in owners.iter().filter(|claim| {
                    claim
                        .rsplit_once(':')
                        .and_then(|(owner, _token)| owner.rsplit_once(':'))
                        .is_some_and(|(claim_name, _generation)| claim_name == name)
                }) {
                    if !claims.contains(claim) {
                        claims.push(claim.clone());
                    }
                }
            }
        }
        claims
    }

    /// Replace cache contents from a successful refresh of the durable
    /// rows. Round 6 item 5: refresh is NOT a persistence acknowledgement —
    /// the pending set is left UNTOUCHED here, and every pending key the
    /// refreshed rows no longer derive KEEPS its current reason in
    /// `entries`, so reads and the drop/rename guards never regress below
    /// the prepublished set. A pending key leaves the cache only through an
    /// explicit [`Self::confirm_pending`] (after the owning mark's SET +
    /// interner persist + flush + refresh all succeeded) or
    /// [`Self::rollback_pending`] (definitive rollback). On persist/flush
    /// FAILURE refresh is never reached, so the last safe snapshot
    /// (including the prepublished keys) stays — protection never
    /// regresses.
    ///
    /// COUNTERFACTUAL: reverting to the pre-round-6 behavior of dropping
    /// unmatched pending keys (and their entries) here turns
    /// `quarantine_cache_replace_clears_persisted_removal_but_keeps_pending`
    /// and `pending_protection_survives_another_subscription_resume` red.
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
        // Pending keys survive untouched; re-add the current reason of every
        // pending key the refreshed rows no longer derive so guards never
        // see the key vanish (see doc above).
        for key in current.pending.keys() {
            if fresh.contains(key) {
                continue;
            }
            if let Some(reason) = current
                .entries
                .get(&key.0)
                .and_then(|repos| repos.get(&key.1))
                .cloned()
            {
                next_entries
                    .entry(key.0.clone())
                    .or_default()
                    .insert(key.1.clone(), reason);
            }
        }
        self.snapshot.store(Arc::new(QuarantineSnapshot {
            entries: next_entries,
            pending: current.pending.clone(),
            derived: fresh,
        }));
    }
    /// Explicit persistence acknowledgement (round 6 item 5): `owner`'s
    /// claims on `keys` leave the pending set; an entry is dropped UNLESS
    /// the last refresh's rows still derive it (then the entry is the
    /// durable truth, not the pending protection) or another subscription
    /// still claims it. Called only after the owning mark's SET + interner
    /// persist + flush + refresh ALL succeeded.
    pub(crate) fn confirm_pending(
        &self,
        owner: &str,
        keys: impl IntoIterator<Item = (String, String)>,
    ) {
        self.resolve_pending(owner, keys, true);
    }
    fn resolve_pending(
        &self,
        owner: &str,
        keys: impl IntoIterator<Item = (String, String)>,
        _confirm: bool,
    ) {
        let current = self.snapshot.load_full();
        let mut next = QuarantineSnapshot {
            entries: current.entries.clone(),
            pending: current.pending.clone(),
            derived: current.derived.clone(),
        };
        for key in keys {
            let last = Self::remove_owner_claims(&mut next, &key, owner);
            if last && !next.derived.contains(&key) {
                if let Some(repos) = next.entries.get_mut(&key.0) {
                    repos.remove(&key.1);
                    if repos.is_empty() {
                        next.entries.remove(&key.0);
                    }
                }
            }
        }
        self.snapshot.store(Arc::new(next));
    }
    /// Definitive documented rollback
    /// `keys` always leave the pending set, but an ENTRY is dropped only
    /// when the last refresh's rows no longer derive it AND no other
    /// subscription still claims it — the durable rows are the source of
    /// truth (mirrors `confirm_pending`). A still-quarantined sibling
    /// subscription covering the same destination keeps its row-derived
    /// entry, and (round 7 item 5) a sibling's PENDING claim keeps the
    /// pending protection even though THIS owner rolled back.
    pub(crate) fn rollback_pending(
        &self,
        owner: &str,
        keys: impl IntoIterator<Item = (String, String)>,
    ) {
        self.resolve_pending(owner, keys, false);
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
