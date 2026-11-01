use super::record::TokenRecord;
use crate::server::auth::token::format::TokenId;
use dashmap::DashMap;
use rustc_hash::FxHasher;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
type FxBuild = BuildHasherDefault<FxHasher>;
type OwnerIds = HashSet<TokenId, FxBuild>;
const GATE_STRIPES: usize = 64;
/// `revoke` uses a Release CAS to publish the first nonzero timestamp, and `is_revoked`
/// uses an Acquire load. Observing that release establishes a happens-before edge for
/// immutable record state and revocation. A reader may load live before publication;
/// any read after `revoke` returns observes the publication because the return follows
/// the Release CAS. A session retaining this Arc reads the same atomic cell.
/// Post-return observation requires caller synchronization (same thread, join, barrier, or channel);
/// Release/Acquire orders visibility but alone does not establish wall-clock synchronization.
/// Stable gate cells are retained per distinct ID for mirror lifetime to prevent
/// per-ID gate identity races; this bounds correctness but allows metadata growth.
/// Correction: fixed bounded stripes now provide stable serialization without per-ID retention.
/// The primary map, owner index, and per-ID gate registry use Fx hashers for fixed-size internal keys.
/// No two DashMap guards are held simultaneously. Gate cells are retained for the mirror lifetime
/// to prevent cell-identity races; metadata grows with distinct IDs observed.
/// Correction: replaced by fixed bounded Fx-hashed stripes; no per-ID gate entries persist.
pub struct TokenState {
    record: TokenRecord,
    revoked: AtomicU64,
}
impl TokenState {
    /// Normalize legacy/invalid zero revocation timestamps to sentinel 1; atomic zero alone means live.
    pub fn new(mut record: TokenRecord) -> Self {
        let initial = match record.revoked_at_ns {
            Some(0) => {
                record.revoked_at_ns = Some(1);
                1
            }
            Some(n) => n,
            None => 0,
        };
        Self {
            record,
            revoked: AtomicU64::new(initial),
        }
    }
    pub fn record(&self) -> &TokenRecord {
        &self.record
    }
    pub fn is_revoked(&self) -> bool {
        self.revoked.load(Ordering::Acquire) != 0
    }
    /// Acquire/release form the publication edge: loads observing this write see revocation.
    pub fn revoke(&self, now: u64) -> bool {
        self.revoked
            .compare_exchange(0, now.max(1), Ordering::Release, Ordering::Relaxed)
            .is_ok()
    }
    pub fn revoked_at_ns(&self) -> Option<u64> {
        match self.revoked.load(Ordering::Acquire) {
            0 => None,
            n => Some(n),
        }
    }
}
impl std::fmt::Debug for TokenState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenState")
            .field("record", &self.record)
            .field("revoked", &self.is_revoked())
            .finish()
    }
}
/// Stable gate cells are retained for the mirror lifetime
/// to prevent cell-identity races; metadata grows with distinct IDs observed.
/// Correction: replaced by fixed bounded Fx-hashed stripes; no per-ID gate entries persist.
/// The original map comment claimed per-ID gate cells, which the bounded stripes below supersede.
/// No two DashMap guards are held simultaneously. Per-ID mutations share a deterministic stripe;
/// collisions only add serialization, and RAII releases the stripe even if mutation unwinds.
/// Sync-only API: each DashMap guard is released before acquiring another; no guard crosses await.
pub struct TokenMirror {
    by_id: DashMap<TokenId, Arc<TokenState>, FxBuild>,
    by_owner: DashMap<[u8; 16], OwnerIds, FxBuild>,
    count: AtomicUsize,
    gates: [AtomicBool; GATE_STRIPES],
}
struct GateGuard<'a>(&'a AtomicBool);
impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Default for TokenMirror {
    fn default() -> Self {
        Self::new()
    }
}
impl TokenMirror {
    pub fn new() -> Self {
        Self {
            by_id: DashMap::with_hasher(FxBuild::default()),
            by_owner: DashMap::with_hasher(FxBuild::default()),
            count: AtomicUsize::new(0),
            gates: std::array::from_fn(|_| AtomicBool::new(false)),
        }
    }
    fn stripe(id: &TokenId) -> usize {
        let mut hasher = FxHasher::default();
        id.hash(&mut hasher);
        hasher.finish() as usize % GATE_STRIPES
    }
    fn gate(&self, id: &TokenId) -> GateGuard<'_> {
        let gate = &self.gates[Self::stripe(id)];
        while gate
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::hint::spin_loop();
        }
        GateGuard(gate)
    }
    fn remove_owner_id(&self, owner: [u8; 16], id: &TokenId) {
        if let dashmap::mapref::entry::Entry::Occupied(mut entry) = self.by_owner.entry(owner) {
            // OccupiedEntry keeps the shard write guard while entry.remove() rechecks emptiness.
            entry.get_mut().remove(id);
            if entry.get().is_empty() {
                entry.remove();
            }
        }
    }
    /// Insert/remove acquire one per-ID atomic gate before any index operation. No map guard is retained across a second map acquisition.
    pub fn insert(&self, record: TokenRecord) -> bool {
        let id = TokenId(record.id);
        let owner = record.owner_user_id;
        let _gate = self.gate(&id);
        if self.by_id.contains_key(&id) {
            return false;
        }
        self.by_owner.entry(owner).or_default().insert(id);
        // The per-ID gate and preceding absence check make replacement impossible (stripe collisions only serialize unrelated IDs).
        self.by_id.insert(id, Arc::new(TokenState::new(record)));
        self.count.fetch_add(1, Ordering::Relaxed);
        true
    }
    pub fn get(&self, id: &TokenId) -> Option<Arc<TokenState>> {
        self.by_id.get(id).map(|v| Arc::clone(v.value()))
    }
    pub fn revoke(&self, id: &TokenId, now: u64) -> bool {
        self.get(id).is_some_and(|s| s.revoke(now))
    }
    pub fn revoke_owner(&self, owner: &[u8; 16], now: u64) -> usize {
        self.ids_of_owner(owner)
            .iter()
            .filter(|id| self.revoke(id, now))
            .count()
    }
    pub fn ids_of_owner(&self, owner: &[u8; 16]) -> Vec<TokenId> {
        self.by_owner
            .get(owner)
            .map(|v| v.iter().copied().collect())
            .unwrap_or_default()
    }
    pub fn remove(&self, id: &TokenId) -> bool {
        let _gate = self.gate(id);
        let Some((_, state)) = self.by_id.remove(id) else {
            return false;
        };
        self.count.fetch_sub(1, Ordering::Relaxed);
        self.remove_owner_id(state.record().owner_user_id, id);
        true
    }
    pub fn load_all(&self, records: impl IntoIterator<Item = TokenRecord>) {
        for record in records {
            self.insert(record);
        }
    }
    pub fn len_hint(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }
}
