//! Two-mode interner wrapper for tx-aware paths.
//!
//! See `docs/dev-artifacts/pre-transactional/03-repo-coordinator.md` §2.3 and D5 in
//! `architectural-decisions.md` for rationale.

use scc::HashMap as SccHashMap;
use shamir_collections::{TFxMap, THasher};
use shamir_storage::error::DbResult;
use shamir_types::core::interner::{Interner, InternerKey};
use std::sync::atomic::{AtomicU64, Ordering};

/// Starting value for overlay ids. Any id >= this value is treated as an
/// unmerged overlay id. On merge it is remapped to a real base id.
///
/// 2^48 is high enough that the real base interner will never reach it in
/// practice, and leaves billions of overlay ids per tx.
pub const OVERLAY_ID_BASE: u64 = 1u64 << 48;

/// Two-mode wrapper over a shared [`Interner`].
///
/// * `Direct` — non-tx path, forwards to `base` directly (zero overhead).
/// * `Layered` — tx path: reads check `base` then `overlay`; new keys are
///   allocated in `overlay` with ids >= [`OVERLAY_ID_BASE`].
pub enum LayeredInterner<'a> {
    Direct(&'a Interner),
    Layered {
        base: &'a Interner,
        overlay: &'a SccHashMap<String, u64, THasher>,
        next_overlay_id: &'a AtomicU64,
    },
}

impl<'a> LayeredInterner<'a> {
    /// cancel-safe: yes — `Direct` path is sync (`touch_ind`). `Layered`
    /// path uses a single `overlay.entry_async`; either the entry insert
    /// completes or the future is dropped with the map unchanged. The
    /// `fetch_add` on `next_overlay_id` only executes on the vacant
    /// branch after `entry_async` resolves, so cancellation cannot leak
    /// an allocated overlay id.
    ///
    /// Return the id for `key`, allocating if necessary.
    ///
    /// * `Direct` → `base.touch_ind`.
    /// * `Layered` → `base.get_ind` first; if absent, inserts into
    ///   `overlay` with an id from `next_overlay_id`.
    pub async fn touch(&self, key: &str) -> u64 {
        match self {
            Self::Direct(base) => base
                .touch_ind(key)
                .expect("Interner::touch_ind is infallible for valid input")
                .key()
                .id(),
            Self::Layered {
                base,
                overlay,
                next_overlay_id,
            } => {
                if let Some(ik) = base.get_ind(key) {
                    return ik.id();
                }
                let entry = overlay.entry_async(key.to_string()).await;
                use scc::hash_map::Entry::{Occupied, Vacant};
                match entry {
                    Occupied(oe) => *oe.get(),
                    Vacant(ve) => {
                        let id = next_overlay_id.fetch_add(1, Ordering::SeqCst);
                        *ve.insert_entry(id).get()
                    }
                }
            }
        }
    }

    /// Sync version of [`touch`] for use in sync code paths
    /// (e.g., `msgpack_value_to_inner_layered`).
    ///
    /// Uses `scc::HashMap::entry` (sync) instead of `entry_async`.
    pub fn touch_sync(&self, key: &str) -> u64 {
        match self {
            Self::Direct(base) => base
                .touch_ind(key)
                .expect("Interner::touch_ind is infallible for valid input")
                .key()
                .id(),
            Self::Layered {
                base,
                overlay,
                next_overlay_id,
            } => {
                if let Some(ik) = base.get_ind(key) {
                    return ik.id();
                }
                let entry = overlay.entry_sync(key.to_string());
                use scc::hash_map::Entry::{Occupied, Vacant};
                match entry {
                    Occupied(oe) => *oe.get(),
                    Vacant(ve) => {
                        let id = next_overlay_id.fetch_add(1, Ordering::SeqCst);
                        *ve.insert_entry(id).get()
                    }
                }
            }
        }
    }

    /// cancel-safe: yes — read-only lookup. `Direct` branch is sync;
    /// `Layered` branch issues at most one `overlay.read_async` which
    /// performs no state mutation.
    ///
    /// Lookup without allocating an id.
    ///
    /// * `Direct` → `base.get_ind`.
    /// * `Layered` → `base.get_ind`, then `overlay.get`.
    pub async fn get_id(&self, key: &str) -> Option<u64> {
        match self {
            Self::Direct(base) => base.get_ind(key).map(|k| k.id()),
            Self::Layered { base, overlay, .. } => {
                if let Some(ik) = base.get_ind(key) {
                    return Some(ik.id());
                }
                overlay.read_async(key, |_, v| *v).await
            }
        }
    }

    /// Reverse lookup: id → string.
    ///
    /// * `Direct` → `base.get_str`.
    /// * `Layered` → if id < [`OVERLAY_ID_BASE`] → base; otherwise linear
    ///   scan of overlay.
    pub fn get_str(&self, id: u64) -> Option<String> {
        match self {
            Self::Direct(base) => base
                .get_str(&InternerKey::new(id))
                .map(|arc| arc.to_string()),
            Self::Layered { base, overlay, .. } => {
                if id < OVERLAY_ID_BASE {
                    base.get_str(&InternerKey::new(id))
                        .map(|arc| arc.to_string())
                } else {
                    let mut found: Option<String> = None;
                    overlay.iter_sync(|k: &String, v: &u64| {
                        if *v == id {
                            found = Some(k.clone());
                            return false;
                        }
                        true
                    });
                    found
                }
            }
        }
    }
}

/// cancel-safe: NO — `overlay.scan_async` builds a pending list, then
/// the loop calls `base.touch_ind` (sync, mutating) per entry while
/// building `remap`. Cancellation mid-loop leaves base with a subset of
/// merged ids and the caller without a complete remap; the partial
/// merge is harmless (touch_ind is idempotent) but the function must
/// be re-run under the same commit_mutex to obtain a usable remap.
///
/// # Calling contract & safety argument
///
/// In the current P2c / lock-free commit path this is called from
/// `pre_commit_prelock`, i.e. OUTSIDE `RepoTxGate::commit_mutex`
/// (the pre-lock phase runs concurrently with other committers —
/// see `pre_commit.rs::pre_commit_prelock`). It is still correct
/// WITHOUT the mutex because the only mutation it performs on `base`
/// is [`Interner::touch_ind`] / `touch_with_id`, which is a CAS-based
/// idempotent insert: a concurrent committer performing the same merge
/// on the same `base` interner either observes the mapping already
/// present (no-op) or inserts it (and the loser's CAS fails harmlessly).
/// The `(name → id)` assignment is deterministic per name, so two
/// concurrent merges of the same overlay entry converge to the SAME
/// base id without coordination. This CAS-idempotency of `touch_ind`
/// is the load-bearing safety property — it is NOT documented as a
/// contract on `Interner` itself, so any future change to `touch_ind`
/// that breaks idempotency would silently break this function.
///
/// The `remap` returned to the caller IS caller-local (built into a
/// fresh `TFxMap` per call), so concurrent callers do not share remap
/// state — each gets its own `{overlay_id → final_base_id}` view.
///
/// Atomically merge `overlay` into `base`. Returns a remap:
/// `overlay_id → final_base_id`. The caller rewrites any bytes
/// referencing overlay ids before flush. Result of
/// [`commit_interner_overlay`]: the id remap and the delta of
/// genuinely new entries inserted into base during merge.
pub struct OverlayCommitResult {
    /// `overlay_id → final_base_id` for every overlay entry.
    pub remap: TFxMap<u64, u64>,
    /// Entries that were **new** to base (not previously present).
    /// Each tuple is `(field_name, base_id)`.
    pub delta: Vec<(String, u64)>,
}

pub async fn commit_interner_overlay(
    base: &Interner,
    overlay: &SccHashMap<String, u64, THasher>,
) -> DbResult<OverlayCommitResult> {
    let mut remap = TFxMap::default();
    let mut delta = Vec::new();
    let mut pending: Vec<(String, u64)> = Vec::new();
    overlay
        .iter_async(|k, v| {
            pending.push((k.clone(), *v));
            true
        })
        .await;

    for (key, overlay_id) in pending {
        let (final_id, is_new) = match base.get_ind(&key) {
            Some(existing) => (existing.id(), false),
            None => {
                let ti = base
                    .touch_ind(&key)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                (ti.key().id(), ti.is_new())
            }
        };
        remap.insert(overlay_id, final_id);
        if is_new {
            delta.push((key, final_id));
        }
    }
    Ok(OverlayCommitResult { remap, delta })
}
