//! WAL manager — appends / clears in-flight transaction markers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use crate::active_key::WalActiveKey;
use crate::wal_entry::{WalEntry, WalOp};
use crate::wal_entry_any::WalEntryAny;
use crate::wal_entry_v2::WalEntryV2;

/// Manages WAL markers for one `info_store`.
pub struct WalManager {
    info_store: Arc<dyn Store>,
    next_txn_id: AtomicU64,
}

impl WalManager {
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        // Seed from system time so two restarts can't collide on
        // ids. Monotonic across one process lifetime is good enough;
        // full uniqueness across crashes is not required because
        // recovery removes markers before normal operation resumes.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Self {
            info_store,
            next_txn_id: AtomicU64::new(seed.max(1)),
        }
    }

    /// Allocate a fresh, process-monotonic txn_id.
    pub fn fresh_txn_id(&self) -> u64 {
        self.next_txn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Build the marker key for one txn.
    fn marker_key(txn_id: u64) -> Bytes {
        WalActiveKey::new(txn_id).to_bytes()
    }

    fn parse_txn_id(key: &[u8]) -> Option<u64> {
        WalActiveKey::parse(key)
    }

    /// Write the in-flight marker for a transaction. Caller must
    /// eventually invoke `commit(txn_id)` once the actual data +
    /// index writes have landed.
    ///
    /// `ops` lists all record-level operations the transaction
    /// intends to perform — recovery uses this to scope its checks.
    pub async fn begin(&self, txn_id: u64, ops: Vec<WalOp>) -> DbResult<()> {
        self.begin_with_delta(txn_id, ops, 0).await
    }

    /// Same as `begin`, but records the net counter delta the
    /// transaction was about to apply. Used by `insert_many`
    /// (delta = +N), `delete_many` (delta = -N), and any future
    /// op that changes the row count.
    pub async fn begin_with_delta(
        &self,
        txn_id: u64,
        ops: Vec<WalOp>,
        counter_delta: i64,
    ) -> DbResult<()> {
        let entry = WalEntry::new_with_delta(txn_id, ops, counter_delta);
        let bytes = bincode::serialize(&entry)
            .map_err(|e| DbError::Codec(format!("WAL serialize: {e}")))?;
        self.info_store
            .set(Self::marker_key(txn_id), Bytes::from(bytes))
            .await?;
        Ok(())
    }

    /// Remove the in-flight marker — call after the transaction's
    /// actual writes have landed durably. Idempotent: removing an
    /// already-absent marker is a no-op.
    pub async fn commit(&self, txn_id: u64) -> DbResult<()> {
        // Ok-value (removed entry) intentionally discarded; ? propagates errors.
        let _ = self.info_store.remove(Self::marker_key(txn_id)).await?;
        Ok(())
    }

    /// Fire-and-forget version of `commit`. Spawns the marker
    /// removal on the tokio runtime and returns the `JoinHandle`
    /// without waiting. The caller can ACK its batch to the client
    /// immediately; if the process dies before the spawned remove
    /// lands, the next-open recovery sees the marker and runs the
    /// idempotent targeted re-apply (records already there, hooks
    /// rewrite the same posting entries).
    ///
    /// Cost saved per batch: one `info_store.remove` await. On
    /// backends with eventual flush (sled / redb-Durability::None
    /// / fjall) this is ~µs and not worth chasing. On per-commit-
    /// fsync backends (persy / nebari / canopy) it's ~5-10 ms on
    /// NTFS — meaningful.
    pub fn commit_async(&self, txn_id: u64) -> tokio::task::JoinHandle<DbResult<()>> {
        let info_store = self.info_store.clone();
        let key = Self::marker_key(txn_id);
        tokio::spawn(async move {
            // Ok-value (removed entry) intentionally discarded; ? propagates errors.
            let _ = info_store.remove(key).await?;
            Ok(())
        })
    }

    /// List every in-flight transaction found on disk. Empty on a
    /// cleanly-shut-down database. Called once on open / startup.
    ///
    /// Returns [`WalEntryAny`] so callers can handle both V1
    /// (non-transactional) and V2 (transactional) entries that share
    /// the same `WalActiveKey` prefix.
    ///
    /// Scan batch size = 1024 — recovery is one-shot; bigger
    /// batches amortise the stream-driver overhead (one Vec
    /// allocation per batch, one await point per batch).
    pub async fn list_inflight(&self) -> DbResult<Vec<WalEntryAny>> {
        use futures::StreamExt;
        let mut out: Vec<WalEntryAny> = Vec::new();
        let prefix = WalActiveKey::scan_prefix();
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                let _txn_id = match Self::parse_txn_id(k.as_ref()) {
                    Some(id) => id,
                    None => continue, // skip foreign keys
                };
                if WalEntryV2::looks_like_v2(&v) {
                    let v2 = WalEntryV2::decode(&v)?;
                    out.push(WalEntryAny::V2(v2));
                } else {
                    let v1: WalEntry = bincode::deserialize(&v)
                        .map_err(|e| DbError::Codec(format!("WAL deserialize: {e}")))?;
                    out.push(WalEntryAny::V1(v1));
                }
            }
        }
        Ok(out)
    }

    /// Convenience: build a `WalOp` list of `RecordCreated` for a
    /// batch of record_ids. Used by `TableManager::insert_many`.
    pub fn ops_record_created(record_ids: &[RecordId]) -> Vec<WalOp> {
        record_ids
            .iter()
            .map(|rid| WalOp::RecordCreated { record_id: *rid })
            .collect()
    }

    pub fn ops_record_deleted(record_ids: &[RecordId]) -> Vec<WalOp> {
        record_ids
            .iter()
            .map(|rid| WalOp::RecordDeleted { record_id: *rid })
            .collect()
    }

    pub fn ops_record_updated(record_ids: &[RecordId]) -> Vec<WalOp> {
        record_ids
            .iter()
            .map(|rid| WalOp::RecordUpdated { record_id: *rid })
            .collect()
    }

    /// Test-only accessor for the underlying info_store. Used by
    /// doctor integration tests to plant deliberate corruption.
    #[doc(hidden)]
    pub fn info_store_for_test(&self) -> &Arc<dyn Store> {
        &self.info_store
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal_entry_v2::WalOpV2;
    use shamir_storage::storage_in_memory::InMemoryStore;

    fn fresh() -> WalManager {
        WalManager::new(Arc::new(InMemoryStore::new()))
    }

    #[tokio::test]
    async fn begin_commit_leaves_no_inflight() {
        let wal = fresh();
        let txn_id = wal.fresh_txn_id();
        let ops = WalManager::ops_record_created(&[RecordId::new(), RecordId::new()]);
        wal.begin(txn_id, ops).await.unwrap();
        let inflight = wal.list_inflight().await.unwrap();
        assert_eq!(inflight.len(), 1);
        wal.commit(txn_id).await.unwrap();
        let inflight = wal.list_inflight().await.unwrap();
        assert!(inflight.is_empty(), "commit must remove the marker");
    }

    #[tokio::test]
    async fn begin_without_commit_visible_after_reopen() {
        // Same `info_store` Arc — emulates re-opening with the same
        // backend instance; the marker survives because info_store
        // does. In a real on-disk backend the marker survives a
        // process restart for the same reason.
        let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let wal1 = WalManager::new(info.clone());
        let txn_id = wal1.fresh_txn_id();
        wal1.begin(
            txn_id,
            vec![WalOp::RecordCreated {
                record_id: RecordId::new(),
            }],
        )
        .await
        .unwrap();
        // No commit.
        let wal2 = WalManager::new(info);
        let inflight = wal2.list_inflight().await.unwrap();
        assert_eq!(inflight.len(), 1);
        assert_eq!(inflight[0].txn_id(), txn_id);
        let WalEntryAny::V1(ref v1) = inflight[0] else {
            panic!("expected V1 entry");
        };
        assert!(matches!(v1.ops[0], WalOp::RecordCreated { .. }));
    }

    #[tokio::test]
    async fn commit_is_idempotent() {
        let wal = fresh();
        let txn_id = wal.fresh_txn_id();
        wal.begin(txn_id, vec![]).await.unwrap();
        wal.commit(txn_id).await.unwrap();
        // Second commit on an already-removed marker — must not error.
        wal.commit(txn_id).await.unwrap();
    }

    #[tokio::test]
    async fn commit_async_eventually_removes_marker() {
        let wal = fresh();
        let txn_id = wal.fresh_txn_id();
        wal.begin(txn_id, vec![]).await.unwrap();
        assert_eq!(wal.list_inflight().await.unwrap().len(), 1);

        let handle = wal.commit_async(txn_id);
        // Marker may or may not be gone yet — we don't block here
        // (the point of commit_async). After awaiting the handle
        // it MUST be gone.
        handle.await.unwrap().unwrap();
        let inflight = wal.list_inflight().await.unwrap();
        assert!(
            inflight.is_empty(),
            "commit_async must remove the marker (got {} inflight)",
            inflight.len()
        );
    }

    #[tokio::test]
    async fn commit_async_is_non_blocking_path() {
        // Confirms commit_async returns a JoinHandle synchronously
        // (i.e. it returns before the spawned task has a chance to
        // do anything). The caller is therefore free to ACK its
        // batch right after, without waiting on the marker remove.
        let wal = fresh();
        let txn_id = wal.fresh_txn_id();
        wal.begin(txn_id, vec![]).await.unwrap();
        let _handle: tokio::task::JoinHandle<DbResult<()>> = wal.commit_async(txn_id);
        // We don't await — the test asserts the synchronous return
        // shape only. The actual removal completes some time later;
        // it's verified in commit_async_eventually_removes_marker.
    }

    #[tokio::test]
    async fn fresh_txn_ids_are_monotonic() {
        let wal = fresh();
        let a = wal.fresh_txn_id();
        let b = wal.fresh_txn_id();
        let c = wal.fresh_txn_id();
        assert!(b > a);
        assert!(c > b);
    }

    #[tokio::test]
    async fn list_inflight_returns_mixed_v1_v2() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let wal = WalManager::new(Arc::clone(&store));

        // Write a V1 entry via existing begin().
        let txn_id_1 = wal.fresh_txn_id();
        wal.begin(
            txn_id_1,
            vec![WalOp::RecordCreated {
                record_id: RecordId::new(),
            }],
        )
        .await
        .unwrap();

        // Write a V2 entry manually (no public API to begin V2 yet —
        // comes in stage 4). Poke the bytes directly to simulate what
        // a future RepoWalManager would do.
        let v2 = WalEntryV2::new(
            wal.fresh_txn_id(),
            0,
            vec![WalOpV2::Delete {
                table_id_interned: 0,
                rid: RecordId::new(),
            }],
        );
        let v2_bytes = v2.encode().unwrap();
        let active_key = WalActiveKey::new(v2.txn_id).to_record_key();
        store.set(active_key, Bytes::from(v2_bytes)).await.unwrap();

        let listed = wal.list_inflight().await.unwrap();
        assert_eq!(listed.len(), 2);
        let mut v1_count = 0;
        let mut v2_count = 0;
        for entry in &listed {
            match entry {
                WalEntryAny::V1(_) => v1_count += 1,
                WalEntryAny::V2(_) => v2_count += 1,
            }
        }
        assert_eq!(v1_count, 1);
        assert_eq!(v2_count, 1);
    }

    #[tokio::test]
    async fn list_inflight_v1_only_after_commit() {
        // Sanity: existing V1-only flow still works (regression guard).
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let wal = WalManager::new(Arc::clone(&store));

        let txn_id = wal.fresh_txn_id();
        wal.begin(
            txn_id,
            vec![WalOp::RecordCreated {
                record_id: RecordId::new(),
            }],
        )
        .await
        .unwrap();

        let listed = wal.list_inflight().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(matches!(listed[0], WalEntryAny::V1(_)));

        wal.commit(txn_id).await.unwrap();
        let listed = wal.list_inflight().await.unwrap();
        assert!(listed.is_empty());
    }
}
