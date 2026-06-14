//! WAL manager — appends / clears in-flight transaction markers.

use std::cell::RefCell;
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

thread_local! {
    /// Scratch buffer reused across `begin_with_delta` calls on the same thread.
    /// Avoids a fresh `Vec<u8>` allocation per WAL marker write.
    static WAL_ENCODE_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(512));
}

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
        // Reuse a thread-local scratch buffer to avoid a fresh Vec allocation
        // per begin call. `serialize_into` writes directly into the cleared
        // buffer; we clone only the final byte slice into an owned `Bytes`.
        let bytes = WAL_ENCODE_BUF.with(|cell| {
            let mut buf = cell.borrow_mut();
            buf.clear();
            bincode::serialize_into(&mut *buf, &entry)
                .map_err(|e| DbError::Codec(format!("WAL serialize: {e}")))?;
            Ok::<Bytes, DbError>(Bytes::copy_from_slice(&buf))
        })?;
        self.info_store.set(Self::marker_key(txn_id), bytes).await?;
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
