//! WAL manager — appends / clears in-flight transaction markers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;

use super::wal_entry::{WalEntry, WalOp};

/// Prefix used for all WAL marker keys in `info_store`.
const ACTIVE_PREFIX: &[u8] = b"__wal_active_";

/// Length of the marker key: prefix + 8-byte big-endian txn_id.
const ACTIVE_KEY_LEN: usize = ACTIVE_PREFIX.len() + 8;

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
        let mut k = Vec::with_capacity(ACTIVE_KEY_LEN);
        k.extend_from_slice(ACTIVE_PREFIX);
        k.extend_from_slice(&txn_id.to_be_bytes());
        Bytes::from(k)
    }

    fn parse_txn_id(key: &[u8]) -> Option<u64> {
        if key.len() != ACTIVE_KEY_LEN || !key.starts_with(ACTIVE_PREFIX) {
            return None;
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&key[ACTIVE_PREFIX.len()..]);
        Some(u64::from_be_bytes(bytes))
    }

    /// Write the in-flight marker for a transaction. Caller must
    /// eventually invoke `commit(txn_id)` once the actual data +
    /// index writes have landed.
    ///
    /// `ops` lists all record-level operations the transaction
    /// intends to perform — recovery uses this to scope its checks.
    pub async fn begin(&self, txn_id: u64, ops: Vec<WalOp>) -> DbResult<()> {
        let entry = WalEntry::new(txn_id, ops);
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
        let _ = self.info_store.remove(Self::marker_key(txn_id)).await?;
        Ok(())
    }

    /// List every in-flight transaction found on disk. Empty on a
    /// cleanly-shut-down database. Called once on open / startup.
    pub async fn list_inflight(&self) -> DbResult<Vec<WalEntry>> {
        use futures::StreamExt;
        let mut out: Vec<WalEntry> = Vec::new();
        let prefix = Bytes::copy_from_slice(ACTIVE_PREFIX);
        let stream = self.info_store.scan_prefix_stream(prefix, 64);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (k, v) in batch? {
                let _txn_id = match Self::parse_txn_id(k.as_ref()) {
                    Some(id) => id,
                    None => continue, // skip foreign keys
                };
                let entry: WalEntry = bincode::deserialize(&v).map_err(|e| {
                    DbError::Codec(format!("WAL deserialize: {e}"))
                })?;
                out.push(entry);
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
        assert_eq!(inflight[0].txn_id, txn_id);
        assert!(matches!(
            inflight[0].ops[0],
            WalOp::RecordCreated { .. }
        ));
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
    async fn fresh_txn_ids_are_monotonic() {
        let wal = fresh();
        let a = wal.fresh_txn_id();
        let b = wal.fresh_txn_id();
        let c = wal.fresh_txn_id();
        assert!(b > a);
        assert!(c > b);
    }
}
