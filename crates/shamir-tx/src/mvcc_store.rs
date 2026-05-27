//! Versioned KV layer over two dumb-KV stores (main + history).
//!
//! [`MvccStore`] wraps two [`Store`] instances:
//! - `main` — the current version of every key (identical to today's
//!   non-tx writes).
//! - `history` — old versions stored under `<key>::0xFF::<version_be>`
//!   keys (see [`version_codec`]).
//!
//! Zero-overhead when no transaction is active: [`MvccStore::set_versioned`]
//! checks `gate.active_snapshots_empty()` and skips history archival
//! entirely, writing directly to `main`.
//!
//! Snapshot reads via [`MvccStore::get_at`] use a fast path (version cache
//! check → main read) and fall back to a history range scan.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use scc::HashMap as SccHashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;

/// Versioned layer over two dumb-KV stores.
///
/// See [module-level documentation](self) for the design rationale.
pub struct MvccStore {
    main: Arc<dyn Store>,
    history: Arc<dyn Store>,
    gate: Arc<RepoTxGate>,
    /// In-memory cache: key → latest committed version.
    /// Cold start: first `get_at` for a key does a range scan, populates cache.
    version_cache: SccHashMap<Bytes, u64>,
}

impl MvccStore {
    /// Create a new MVCC store from two backing stores and a gate.
    pub fn new(main: Arc<dyn Store>, history: Arc<dyn Store>, gate: Arc<RepoTxGate>) -> Self {
        Self {
            main,
            history,
            gate,
            version_cache: SccHashMap::new(),
        }
    }

    /// Non-tx write. If active snapshots exist, saves the old value
    /// in history before overwriting main.
    pub async fn set_versioned(&self, key: Bytes, value: Bytes) -> DbResult<()> {
        if self.gate.active_snapshots_empty() {
            self.main.set(key, value).await?;
            return Ok(());
        }
        // Archive old value if it exists.
        if let Ok(old) = self.main.get(key.clone()).await {
            let cur_v = self.current_version(&key);
            let h_key = encode_version_key(&key, cur_v);
            self.history.set(h_key, old).await?;
        }
        self.main.set(key.clone(), value).await?;
        let new_v = self.gate.assign_next_version();
        self.version_cache.entry(key).insert_entry(new_v);
        Ok(())
    }

    /// Non-tx delete. Similar to `set_versioned` — archives old value
    /// if snapshots are active.
    pub async fn delete_versioned(&self, key: Bytes) -> DbResult<()> {
        if !self.gate.active_snapshots_empty() {
            if let Ok(old) = self.main.get(key.clone()).await {
                let cur_v = self.current_version(&key);
                let h_key = encode_version_key(&key, cur_v);
                self.history.set(h_key, old).await?;
            }
        }
        let _ = self.main.remove(key.clone()).await;
        if !self.gate.active_snapshots_empty() {
            let new_v = self.gate.assign_next_version();
            self.version_cache.entry(key).insert_entry(new_v);
        }
        Ok(())
    }

    /// Snapshot read: return the value visible at `snapshot_version`.
    ///
    /// Fast path: if version_cache says current version ≤ snapshot →
    /// return `main.get(key)`.
    /// Slow path: range scan history `[key::0, key::snapshot]`, take last.
    pub async fn get_at(&self, key: &[u8], snapshot_version: u64) -> DbResult<Option<Bytes>> {
        let cur_v = self.current_version(key);
        if cur_v <= snapshot_version {
            return match self.main.get(Bytes::copy_from_slice(key)).await {
                Ok(v) => Ok(Some(v)),
                Err(DbError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            };
        }
        self.scan_history_for_version(key, snapshot_version).await
    }

    /// Direct access to main store (for non-tx reads).
    pub fn main_store(&self) -> &Arc<dyn Store> {
        &self.main
    }

    /// Direct access to history store (for GC, recovery).
    pub fn history_store(&self) -> &Arc<dyn Store> {
        &self.history
    }

    /// Look up the latest committed version for `key` in the in-memory cache.
    /// Returns 0 if the key is not cached (meaning "initial / no version tracked").
    fn current_version(&self, key: &[u8]) -> u64 {
        let key_bytes = Bytes::copy_from_slice(key);
        self.version_cache.read(&key_bytes, |_, v| *v).unwrap_or(0)
    }

    /// Slow path: range scan history for the latest version ≤ `snapshot`.
    async fn scan_history_for_version(&self, key: &[u8], snapshot: u64) -> DbResult<Option<Bytes>> {
        let lo = encode_version_key(key, 0);
        let hi = encode_version_key(key, snapshot);
        let stream = self.history.iter_range_stream(Some(lo), Some(hi), 64);
        futures::pin_mut!(stream);
        let mut latest: Option<Bytes> = None;
        while let Some(batch) = stream.next().await {
            for (_, val) in batch? {
                latest = Some(val);
            }
        }
        Ok(latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryStore;

    fn make_gate() -> Arc<RepoTxGate> {
        Arc::new(RepoTxGate::fresh())
    }

    fn make_mvcc() -> MvccStore {
        let gate = make_gate();
        MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryStore::new()),
            gate,
        )
    }

    fn make_mvcc_with_gate(gate: Arc<RepoTxGate>) -> MvccStore {
        MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryStore::new()),
            gate,
        )
    }

    #[tokio::test]
    async fn set_without_snapshots_skips_history() {
        let mvcc = make_mvcc();
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        // main has the value
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v1"));

        // history is empty
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            let batch = batch.unwrap();
            assert!(batch.is_empty(), "history should be empty");
        }
    }

    #[tokio::test]
    async fn set_with_snapshot_archives_old_value() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // Open a snapshot so active_snapshots is non-empty.
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // main has v2
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v2"));

        // history contains v1 under a version key
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                let (orig_key, _ver) = crate::version_codec::decode_version_key(&hk).unwrap();
                assert_eq!(orig_key, &b"k1"[..]);
                assert_eq!(hv, Bytes::from("v1"));
                found = true;
            }
        }
        assert!(found, "history should contain archived v1");
    }

    #[tokio::test]
    async fn get_at_current_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        let v = gate.assign_next_version();
        // snapshot_version >> v → fast path returns from main
        let result = mvcc.get_at(b"k1", v + 100).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));
    }

    #[tokio::test]
    async fn get_at_old_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        // v1 written at version 1
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // v2 written at version 2
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // Query at snapshot between v1 and v2 — should find v1 in history.
        // After two set_versioned calls, version_cache[k1] = 2.
        // get_at(snapshot=1) → cur_v(2) > 1 → slow path → scan history.
        let result = mvcc.get_at(b"k1", 1).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));
    }

    #[tokio::test]
    async fn get_at_missing_key() {
        let mvcc = make_mvcc();
        let result = mvcc.get_at(b"nonexistent", 999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_versioned_archives() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.delete_versioned(key.clone()).await.unwrap();

        // main no longer has the key
        assert!(mvcc.main.get(key).await.is_err());

        // history contains v1
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("v1"));
                found = true;
            }
        }
        assert!(found, "history should contain archived v1 after delete");
    }

    #[tokio::test]
    async fn get_at_after_delete() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        // v1 at version 1
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // delete at version 2
        mvcc.delete_versioned(key.clone()).await.unwrap();

        // get_at between v1 and delete → slow path → v1 from history
        let result = mvcc.get_at(b"k1", 1).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));

        // get_at after delete → fast path (cur_v=2 <= 15) → main.get → NotFound → None
        let result = mvcc.get_at(b"k1", 15).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn zero_overhead_no_snapshots() {
        let mvcc = make_mvcc();
        for i in 0..100u32 {
            let key = Bytes::copy_from_slice(&i.to_be_bytes());
            mvcc.set_versioned(key, Bytes::from("val")).await.unwrap();
        }

        // history should be empty — no snapshots were open
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        assert_eq!(
            count, 0,
            "history should have zero records without snapshots"
        );
    }

    #[tokio::test]
    async fn version_cache_populated_on_set() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        let cached = mvcc.version_cache.read(&key, |_, v| *v);
        assert!(
            cached.is_some(),
            "version_cache should contain key after set"
        );
        assert!(cached.unwrap() > 0, "version should be > 0");
    }

    #[tokio::test]
    async fn main_store_accessor() {
        let main: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = make_gate();

        let mvcc = MvccStore::new(Arc::clone(&main), Arc::clone(&history), gate);

        // Verify that main_store() returns the same Arc
        assert!(Arc::ptr_eq(&main, mvcc.main_store()));
        assert!(Arc::ptr_eq(&history, mvcc.history_store()));
    }
}
