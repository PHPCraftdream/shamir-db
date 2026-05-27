//! Repo-level WAL manager — one `WalEntryV2` per transaction/batch.
//!
//! Lives alongside the per-table [`shamir_wal::WalManager`] (V1 back-compat).
//! Transactional writes go through `RepoWalManager`; non-tx writes stay on V1.
//!
//! Shares the same [`WalActiveKey`] physical prefix (`__wal_active_`) — V1 and
//! V2 entries coexist in one keyspace, distinguished by value magic bytes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_wal::{WalActiveKey, WalEntryV2};

/// Repo-scoped WAL — one entry per tx/batch, covering all tables.
///
/// Markers live in the same `info_store` as per-table WAL (V1).
/// [`list_inflight`](Self::list_inflight) filters by V2 magic prefix,
/// skipping V1 entries silently.
pub struct RepoWalManager {
    info_store: Arc<dyn Store>,
    next_txn_id: AtomicU64,
}

impl RepoWalManager {
    /// Create a new `RepoWalManager`.
    ///
    /// `initial_txn_id` is seeded from recovery:
    /// `max(persisted_next_tx_id, max_inflight_txn_id + 1)`.
    pub fn new(info_store: Arc<dyn Store>, initial_txn_id: u64) -> Self {
        Self {
            info_store,
            next_txn_id: AtomicU64::new(initial_txn_id),
        }
    }

    /// Allocate the next monotonic txn_id.
    pub fn fresh_txn_id(&self) -> u64 {
        self.next_txn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Write a V2 entry under its `WalActiveKey`.
    ///
    /// This is the intent marker — if a crash happens before
    /// [`commit`](Self::commit), recovery replays this entry.
    pub async fn begin(&self, entry: WalEntryV2) -> DbResult<()> {
        let encoded = entry.encode()?;
        self.info_store
            .set(WalActiveKey::new(entry.txn_id).to_bytes(), encoded.into())
            .await?;
        Ok(())
    }

    /// Remove the marker — tx writes have landed durably. Idempotent.
    pub async fn commit(&self, txn_id: u64) -> DbResult<()> {
        let _ = self
            .info_store
            .remove(WalActiveKey::new(txn_id).to_bytes())
            .await?;
        Ok(())
    }

    /// Fire-and-forget commit. Returns a [`tokio::task::JoinHandle`].
    pub fn commit_async(&self, txn_id: u64) -> tokio::task::JoinHandle<DbResult<()>> {
        let info_store = self.info_store.clone();
        let key = WalActiveKey::new(txn_id).to_bytes();
        tokio::spawn(async move {
            let _ = info_store.remove(key).await?;
            Ok(())
        })
    }

    /// List V2 entries that survived a crash (no commit marker removed).
    ///
    /// Scans all `WalActiveKey` entries, sniffs V2 magic, decodes.
    /// V1 entries (belonging to per-table `WalManager`) are skipped.
    pub async fn list_inflight(&self) -> DbResult<Vec<WalEntryV2>> {
        let mut out = Vec::new();
        let prefix = WalActiveKey::scan_prefix();
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (_k, v) in batch? {
                if WalEntryV2::looks_like_v2(&v) {
                    out.push(WalEntryV2::decode(&v)?);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_types::types::record_id::RecordId;
    use shamir_wal::{WalEntry, WalOp};

    fn rid(n: u8) -> RecordId {
        let mut a = [0u8; 16];
        a[15] = n;
        RecordId(a)
    }

    fn make_store() -> Arc<dyn Store> {
        Arc::new(InMemoryStore::new())
    }

    fn make_manager(store: &Arc<dyn Store>) -> RepoWalManager {
        RepoWalManager::new(store.clone(), 1000)
    }

    fn simple_entry(txn_id: u64) -> WalEntryV2 {
        WalEntryV2::new(
            txn_id,
            0,
            vec![shamir_wal::WalOpV2::Put {
                rid: rid(1),
                body: Bytes::from_static(b"hello"),
            }],
        )
    }

    #[tokio::test]
    async fn begin_commit_no_inflight() {
        let store = make_store();
        let mgr = make_manager(&store);
        let entry = simple_entry(100);

        mgr.begin(entry).await.unwrap();
        let inflight = mgr.list_inflight().await.unwrap();
        assert_eq!(inflight.len(), 1);
        assert_eq!(inflight[0].txn_id, 100);

        mgr.commit(100).await.unwrap();
        let inflight = mgr.list_inflight().await.unwrap();
        assert!(inflight.is_empty(), "commit must remove the marker");
    }

    #[tokio::test]
    async fn begin_without_commit_survives_reopen() {
        let store = make_store();
        let mgr1 = make_manager(&store);
        let entry = simple_entry(200);

        mgr1.begin(entry.clone()).await.unwrap();

        let mgr2 = make_manager(&store);
        let inflight = mgr2.list_inflight().await.unwrap();
        assert_eq!(inflight.len(), 1);
        assert_eq!(inflight[0].txn_id, 200);
        assert_eq!(inflight[0].ops.len(), 1);
    }

    #[tokio::test]
    async fn commit_is_idempotent() {
        let store = make_store();
        let mgr = make_manager(&store);
        mgr.begin(simple_entry(300)).await.unwrap();

        mgr.commit(300).await.unwrap();
        mgr.commit(300).await.unwrap();

        let inflight = mgr.list_inflight().await.unwrap();
        assert!(inflight.is_empty());
    }

    #[tokio::test]
    async fn commit_async_removes_marker() {
        let store = make_store();
        let mgr = make_manager(&store);
        mgr.begin(simple_entry(400)).await.unwrap();
        assert_eq!(mgr.list_inflight().await.unwrap().len(), 1);

        let handle = mgr.commit_async(400);
        handle.await.unwrap().unwrap();

        let inflight = mgr.list_inflight().await.unwrap();
        assert!(
            inflight.is_empty(),
            "commit_async must remove the marker (got {} inflight)",
            inflight.len()
        );
    }

    #[tokio::test]
    async fn list_inflight_skips_v1_entries() {
        let store = make_store();
        let mgr = make_manager(&store);

        // Write a V1 entry manually (as per-table WalManager would).
        let v1_entry = WalEntry::new(
            10,
            vec![WalOp::RecordCreated {
                record_id: RecordId::new(),
            }],
        );
        let v1_bytes = bincode::serialize(&v1_entry).expect("v1 serialize");
        store
            .set(WalActiveKey::new(10).to_bytes(), Bytes::from(v1_bytes))
            .await
            .unwrap();

        // Write a V2 entry through RepoWalManager.
        mgr.begin(simple_entry(500)).await.unwrap();

        let inflight = mgr.list_inflight().await.unwrap();
        assert_eq!(inflight.len(), 1, "should only see the V2 entry");
        assert_eq!(inflight[0].txn_id, 500);
    }

    #[tokio::test]
    async fn fresh_txn_ids_monotonic() {
        let store = make_store();
        let mgr = make_manager(&store);
        let a = mgr.fresh_txn_id();
        let b = mgr.fresh_txn_id();
        let c = mgr.fresh_txn_id();
        assert!(a < b, "{a} should be < {b}");
        assert!(b < c, "{b} should be < {c}");
    }

    #[tokio::test]
    async fn begin_multiple_then_list() {
        let store = make_store();
        let mgr = make_manager(&store);

        mgr.begin(simple_entry(600)).await.unwrap();
        mgr.begin(simple_entry(601)).await.unwrap();
        mgr.begin(simple_entry(602)).await.unwrap();

        let mut ids: Vec<u64> = mgr
            .list_inflight()
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.txn_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![600, 601, 602]);

        mgr.commit(601).await.unwrap();
        let mut ids: Vec<u64> = mgr
            .list_inflight()
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.txn_id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec![600, 602]);

        mgr.commit(600).await.unwrap();
        mgr.commit(602).await.unwrap();
        assert!(mgr.list_inflight().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recovery_round_trip_all_op_variants() {
        let store = make_store();
        let mgr = make_manager(&store);

        let entry = WalEntryV2 {
            txn_id: 999,
            repo_id_interned: 42,
            started_at_ns: 1_000_000,
            ops: vec![
                shamir_wal::WalOpV2::Put {
                    rid: rid(1),
                    body: Bytes::from_static(b"record-body"),
                },
                shamir_wal::WalOpV2::Delete { rid: rid(2) },
                shamir_wal::WalOpV2::IndexPut {
                    idx_id: 11,
                    key: Bytes::from_static(b"idx-key"),
                    value: Bytes::from_static(b"idx-val"),
                },
                shamir_wal::WalOpV2::IndexDel {
                    idx_id: 11,
                    key: Bytes::from_static(b"idx-key-del"),
                },
                shamir_wal::WalOpV2::InternerOverlayMerge {
                    entries: vec![(100, "email".into()), (101, "score".into())],
                },
                shamir_wal::WalOpV2::CounterDelta {
                    table_id_interned: 5,
                    delta: -3,
                },
            ],
        };

        mgr.begin(entry.clone()).await.unwrap();

        let inflight = mgr.list_inflight().await.unwrap();
        assert_eq!(inflight.len(), 1);
        assert_eq!(
            inflight[0], entry,
            "round-tripped entry must match original"
        );
    }
}
