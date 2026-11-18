//! Admission and counter accounting for token-authenticated sessions.

use crate::server::auth::token::TokenId;
use crate::server::state::origin::SessionOrigin;
use crate::server::state::session::{
    mint_activity_handle, new_activity_projection, projection_snapshot, projection_update,
    ActivityProjection, Session,
};
use dashmap::DashMap;
use rustc_hash::FxHasher;
use std::hash::{BuildHasherDefault, Hash};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
type FxBuild = BuildHasherDefault<FxHasher>;
/// Maximum simultaneous token sessions enforced by admission.
#[derive(Debug, Clone, Copy)]
pub struct TokenSocketCaps {
    /// Maximum token sessions across all owners for one token.
    pub per_token: usize,
    /// Maximum token sessions across all tokens for one owner.
    pub per_owner: usize,
}
/// Reason token-session admission was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenAdmitReject {
    /// The token's simultaneous-session limit was reached.
    #[error("per-token cap reached")]
    PerTokenCap,
    /// The owner's simultaneous-session limit was reached.
    #[error("per-owner cap reached")]
    PerOwnerCap,
    /// The session identifier is already stored.
    #[error("duplicate session id")]
    DuplicateSid,
    /// The supplied session did not originate from a token.
    #[error("not a token session")]
    NotTokenSession,
}
// Stable Arc cells avoid racing counter identity; distinct keys remain allocated for store lifetime.
// Correction: leased cells are pruned after a zero-count operation finishes under exclusive entry access.

struct CounterCell {
    count: AtomicUsize,
    leases: AtomicUsize,
}
struct CounterLease<'a, K: Eq + Hash + Clone> {
    map: &'a DashMap<K, Arc<CounterCell>, FxBuild>,
    key: K,
    cell: Arc<CounterCell>,
}
impl<'a, K: Eq + Hash + Clone> CounterLease<'a, K> {
    fn acquire(map: &'a DashMap<K, Arc<CounterCell>, FxBuild>, key: K) -> Self {
        let entry = map.entry(key.clone()).or_insert_with(|| {
            Arc::new(CounterCell {
                count: AtomicUsize::new(0),
                leases: AtomicUsize::new(0),
            })
        });
        let cell = Arc::clone(entry.value());
        cell.leases.fetch_add(1, Ordering::AcqRel);
        drop(entry);
        Self { map, key, cell }
    }
}
impl<K: Eq + Hash + Clone> Drop for CounterLease<'_, K> {
    fn drop(&mut self) {
        let previous = self.cell.leases.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        if previous != 1 {
            return;
        }
        // The exclusive map entry guard excludes new acquisitions during identity/zero checks.
        if let dashmap::mapref::entry::Entry::Occupied(entry) = self.map.entry(self.key.clone()) {
            if Arc::ptr_eq(entry.get(), &self.cell)
                && self.cell.leases.load(Ordering::Acquire) == 0
                && self.cell.count.load(Ordering::Acquire) == 0
            {
                entry.remove();
            }
        }
    }
}
/// Stores token-authenticated sessions and enforces per-token and per-owner caps.
pub struct TokenSessionStore {
    sessions: DashMap<[u8; 32], Arc<Session>, FxBuild>,
    token_counts: DashMap<TokenId, Arc<CounterCell>, FxBuild>,
    owner_counts: DashMap<[u8; 16], Arc<CounterCell>, FxBuild>,
    /// O(1) mirror of `sessions.len()` feeding activity gauges (same
    /// accounting as `SessionStore::activity_len`).
    activity_len: AtomicUsize,
    /// Lock-free sharded `ArcSwap` projection for the activity view, same
    /// scheme as `SessionStore` (see `projection_update`/`projection_snapshot`
    /// in `state/session.rs` — publishes under the shard write guard,
    /// removals just after their guard is released, ptr_eq-guarded). Handles
    /// come from the same process-wide minter, so all stored sessions (SCRAM
    /// and token) have unique operator-facing identities; caps and admission
    /// are unchanged.
    activity_projection: ActivityProjection,
}
// Synchronous-only API: each DashMap guard is dropped before acquiring another; no guard crosses await.
impl Default for TokenSessionStore {
    fn default() -> Self {
        Self::new()
    }
}
impl TokenSessionStore {
    /// Creates an empty token-session store.
    pub fn new() -> Self {
        Self {
            sessions: DashMap::with_hasher(FxBuild::default()),
            token_counts: DashMap::with_hasher(FxBuild::default()),
            owner_counts: DashMap::with_hasher(FxBuild::default()),
            activity_len: AtomicUsize::new(0),
            activity_projection: new_activity_projection(),
        }
    }
    fn reserve(counter: &AtomicUsize, cap: usize) -> bool {
        let mut current = counter.load(Ordering::Acquire);
        loop {
            if current >= cap {
                return false;
            }
            let Some(next) = current.checked_add(1) else {
                return false;
            };
            match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }
    /// Admits a token session under `sid` if both caps and the identifier permit it.
    pub fn admit(
        &self,
        sid: [u8; 32],
        mut session: Session,
        caps: TokenSocketCaps,
    ) -> Result<Arc<Session>, TokenAdmitReject> {
        let origin = match &session.origin {
            SessionOrigin::Token(origin) => origin,
            SessionOrigin::Scram => return Err(TokenAdmitReject::NotTokenSession),
        };
        let token_count = CounterLease::acquire(&self.token_counts, origin.token_id());
        if !Self::reserve(&token_count.cell.count, caps.per_token) {
            return Err(TokenAdmitReject::PerTokenCap);
        }
        let owner_count = CounterLease::acquire(&self.owner_counts, session.user_id);
        if !Self::reserve(&owner_count.cell.count, caps.per_owner) {
            token_count.cell.count.fetch_sub(1, Ordering::AcqRel);
            return Err(TokenAdmitReject::PerOwnerCap);
        }
        session.session_id = sid;
        let session = Arc::new(session);
        let insertion = match self.sessions.entry(sid) {
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                // Stamp BEFORE publication under this shard write guard (same
                // scheme as `SessionStore::admit`); projection + mirror +1
                // before visibility. A wasted mint on DuplicateSid never
                // publishes, so uniqueness/monotonicity are unaffected.
                session.stamp_handle(mint_activity_handle());
                self.activity_len.fetch_add(1, Ordering::Relaxed);
                projection_update(&self.activity_projection, &sid, None, Some(&session));
                entry.insert(Arc::clone(&session));
                Ok(Arc::clone(&session))
            }
            dashmap::mapref::entry::Entry::Occupied(_) => Err(TokenAdmitReject::DuplicateSid),
        };
        if insertion.is_err() {
            token_count.cell.count.fetch_sub(1, Ordering::AcqRel);
            owner_count.cell.count.fetch_sub(1, Ordering::AcqRel);
        }
        insertion
    }
    /// Removes the session identified by `sid`, releasing its token and owner counts.
    pub fn remove(&self, sid: &[u8; 32]) -> bool {
        let Some((_, session)) = self.sessions.remove(sid) else {
            return false;
        };
        self.activity_len.fetch_sub(1, Ordering::Relaxed);
        projection_update(&self.activity_projection, sid, Some(&session), None);
        let SessionOrigin::Token(origin) = &session.origin else {
            debug_assert!(false, "only token-origin sessions enter TokenSessionStore");
            return false;
        };
        let token = CounterLease::acquire(&self.token_counts, origin.token_id());
        let owner = CounterLease::acquire(&self.owner_counts, session.user_id);
        token.cell.count.fetch_sub(1, Ordering::AcqRel);
        owner.cell.count.fetch_sub(1, Ordering::AcqRel);
        true
    }
    /// Returns a stored session for `sid`, if present.
    pub fn get(&self, sid: &[u8; 32]) -> Option<Arc<Session>> {
        self.sessions
            .get(sid)
            .map(|entry| Arc::clone(entry.value()))
    }
    /// Returns all stored sessions originating from token `id`.
    pub fn sessions_of_token(&self, id: &TokenId) -> Vec<Arc<Session>> {
        self.sessions
            .iter()
            .filter_map(|entry| match &entry.origin {
                SessionOrigin::Token(origin) if &origin.token_id() == id => {
                    Some(Arc::clone(entry.value()))
                }
                _ => None,
            })
            .collect()
    }
    /// Returns the number of allocated token and owner counter cells for tests.
    #[cfg(test)]
    pub(crate) fn counter_cells_hint(&self) -> (usize, usize) {
        (self.token_counts.len(), self.owner_counts.len())
    }
    /// Returns the current number of sessions for token `id`.
    pub fn count_for_token(&self, id: &TokenId) -> usize {
        self.token_counts
            .get(id)
            .map(|entry| entry.count.load(Ordering::Acquire))
            .unwrap_or(0)
    }
    /// Returns the current number of sessions owned by `id`.
    pub fn count_for_owner(&self, id: &[u8; 16]) -> usize {
        self.owner_counts
            .get(id)
            .map(|entry| entry.count.load(Ordering::Acquire))
            .unwrap_or(0)
    }

    /// Number of currently-live token sessions. O(1): reads the mirror
    /// atomics maintained at every `sessions` mutation (see
    /// `SessionStore::len` for the accounting contract).
    pub fn len(&self) -> usize {
        self.activity_len.load(Ordering::Relaxed)
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lock-free copy-only activity snapshot of all live token sessions
    /// (R4b-1), identical contract to [`SessionStore::activity_snapshot`]:
    /// served from the sharded `ArcSwap` projection — atomic loads and `Arc`
    /// clones only, no map guard, momentarily torn across shards (D4),
    /// gauge-balanced after quiescence (`len()` equality).
    pub fn activity_snapshot(&self) -> Vec<Arc<Session>> {
        projection_snapshot(&self.activity_projection)
    }
}
