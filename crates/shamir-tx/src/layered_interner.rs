//! Two-mode interner wrapper for tx-aware paths.
//!
//! See `docs/pre-transactional/03-repo-coordinator.md` §2.3 and D5 in
//! `architectural-decisions.md` for rationale.

use scc::HashMap as SccHashMap;
use shamir_storage::error::DbResult;
use shamir_types::core::interner::{Interner, InternerKey};
use std::collections::HashMap;
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
        overlay: &'a SccHashMap<String, u64>,
        next_overlay_id: &'a AtomicU64,
    },
}

impl<'a> LayeredInterner<'a> {
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
    /// (e.g., `json_value_to_inner_layered`).
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
                let entry = overlay.entry(key.to_string());
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
                .map(|uk: shamir_types::core::interner::UserKey| uk.as_str().to_string()),
            Self::Layered { base, overlay, .. } => {
                if id < OVERLAY_ID_BASE {
                    base.get_str(&InternerKey::new(id))
                        .map(|uk: shamir_types::core::interner::UserKey| uk.as_str().to_string())
                } else {
                    let mut found: Option<String> = None;
                    overlay.scan(|k: &String, v: &u64| {
                        if *v == id {
                            found = Some(k.clone());
                        }
                    });
                    found
                }
            }
        }
    }
}

/// Atomically merge `overlay` into `base`.
///
/// Must be called under `RepoTxGate::commit_mutex` — no internal
/// synchronisation. Returns a remap: `overlay_id → final_base_id`.
/// The caller rewrites any bytes referencing overlay ids before flush.
pub async fn commit_interner_overlay(
    base: &Interner,
    overlay: &SccHashMap<String, u64>,
) -> DbResult<HashMap<u64, u64>> {
    let mut remap = HashMap::new();
    let mut pending: Vec<(String, u64)> = Vec::new();
    overlay
        .scan_async(|k, v| pending.push((k.clone(), *v)))
        .await;

    for (key, overlay_id) in pending {
        let final_id = match base.get_ind(&key) {
            Some(existing) => existing.id(),
            None => base
                .touch_ind(&key)
                .map(|ti| ti.key().id())
                .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?,
        };
        remap.insert(overlay_id, final_id);
    }
    Ok(remap)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `(base_id, overlay, next_overlay_id, layered_interner)`.
    /// The caller must keep `overlay` and `next` alive for the lifetime
    /// of `layered_interner`.
    fn make_layered<'a>(
        base: &'a Interner,
        overlay: &'a SccHashMap<String, u64>,
        next: &'a AtomicU64,
    ) -> LayeredInterner<'a> {
        LayeredInterner::Layered {
            base,
            overlay,
            next_overlay_id: next,
        }
    }

    #[test]
    fn touch_sync_same_as_async() {
        let base = Interner::new();
        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);
        let li = make_layered(&base, &overlay, &next);

        let id = li.touch_sync("sync_key");
        assert!(id >= OVERLAY_ID_BASE);

        // Same key returns same id
        let id2 = li.touch_sync("sync_key");
        assert_eq!(id, id2);
    }

    #[test]
    fn touch_sync_direct_returns_base_id() {
        let base = Interner::new();
        let li = LayeredInterner::Direct(&base);
        let id = li.touch_sync("hello");
        assert!(id < OVERLAY_ID_BASE);
        let got = base.get_ind("hello").expect("should exist in base");
        assert_eq!(got.id(), id);
    }

    #[tokio::test]
    async fn direct_mode_no_overhead() {
        let base = Interner::new();
        let li = LayeredInterner::Direct(&base);

        let id = li.touch("hello").await;
        assert!(id < OVERLAY_ID_BASE);

        let got = base.get_ind("hello").expect("should exist in base");
        assert_eq!(got.id(), id);
    }

    #[tokio::test]
    async fn layered_touch_new_goes_to_overlay() {
        let base = Interner::new();
        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);
        let li = make_layered(&base, &overlay, &next);

        let id = li.touch("brand_new_key").await;
        assert!(
            id >= OVERLAY_ID_BASE,
            "overlay id must be >= OVERLAY_ID_BASE"
        );

        assert!(
            base.get_ind("brand_new_key").is_none(),
            "base must not know the key yet"
        );
        assert!(
            overlay.read_async("brand_new_key", |_, v| *v).await == Some(id),
            "overlay should contain the key"
        );
    }

    #[tokio::test]
    async fn layered_touch_existing_in_base_returns_base_id() {
        let base = Interner::new();
        let base_id = base
            .touch_ind("foo")
            .expect("touch_ind succeeds")
            .key()
            .id();

        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);
        let li = make_layered(&base, &overlay, &next);
        let id = li.touch("foo").await;
        assert_eq!(id, base_id);
        assert!(id < OVERLAY_ID_BASE);
    }

    #[tokio::test]
    async fn layered_touch_repeat_returns_same_overlay_id() {
        let base = Interner::new();
        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);
        let li = make_layered(&base, &overlay, &next);

        let id1 = li.touch("bar").await;
        let id2 = li.touch("bar").await;
        assert_eq!(id1, id2);
        assert!(id1 >= OVERLAY_ID_BASE);
    }

    #[tokio::test]
    async fn get_id_does_not_allocate() {
        let base = Interner::new();
        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);
        let li = make_layered(&base, &overlay, &next);

        assert!(li.get_id("unknown").await.is_none());

        assert!(base.get_ind("unknown").is_none());
        assert!(overlay.is_empty());
    }

    #[tokio::test]
    async fn get_str_reads_base_and_overlay() {
        let base = Interner::new();
        let base_id = base
            .touch_ind("foo")
            .expect("touch_ind succeeds")
            .key()
            .id();

        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);
        let li = make_layered(&base, &overlay, &next);
        let overlay_id = li.touch("bar").await;

        assert_eq!(li.get_str(base_id), Some("foo".to_string()));
        assert_eq!(li.get_str(overlay_id), Some("bar".to_string()));
    }

    #[tokio::test]
    async fn commit_overlay_merges_into_base() {
        let base = Interner::new();
        let overlay = SccHashMap::new();
        let next = AtomicU64::new(OVERLAY_ID_BASE);

        let overlay_a = next.fetch_add(1, Ordering::SeqCst);
        overlay
            .insert_async("a".to_string(), overlay_a)
            .await
            .unwrap();
        let overlay_b = next.fetch_add(1, Ordering::SeqCst);
        overlay
            .insert_async("b".to_string(), overlay_b)
            .await
            .unwrap();

        let remap = commit_interner_overlay(&base, &overlay).await.unwrap();
        assert_eq!(remap.len(), 2);

        let final_a = base.get_ind("a").expect("a should be in base").id();
        let final_b = base.get_ind("b").expect("b should be in base").id();
        assert_eq!(remap[&overlay_a], final_a);
        assert_eq!(remap[&overlay_b], final_b);
    }

    #[tokio::test]
    async fn commit_overlay_with_race_uses_existing_base_id() {
        let base = Interner::new();
        let existing = base
            .touch_ind("foo")
            .expect("touch_ind succeeds")
            .key()
            .id();

        let overlay = SccHashMap::new();
        let overlay_id: u64 = OVERLAY_ID_BASE + 99;
        overlay
            .insert_async("foo".to_string(), overlay_id)
            .await
            .unwrap();

        let remap = commit_interner_overlay(&base, &overlay).await.unwrap();
        assert_eq!(remap[&overlay_id], existing);
    }

    #[tokio::test]
    async fn commit_overlay_empty_is_noop() {
        let base = Interner::new();
        let overlay = SccHashMap::new();
        let base_len = base.len();

        let remap = commit_interner_overlay(&base, &overlay).await.unwrap();
        assert!(remap.is_empty());
        assert_eq!(base.len(), base_len);
    }
}
