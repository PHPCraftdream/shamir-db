use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::migration::shadow_key::ShadowKey;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShadowOp {
    Put {
        record_id: RecordId,
        #[serde(with = "serde_bytes")]
        value: Vec<u8>,
    },
    Delete {
        record_id: RecordId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowEntry {
    pub lsn: u64,
    pub op: ShadowOp,
}

/// Current version byte for the `ShadowEntry` wire format. Bump this
/// whenever `ShadowEntry`/`ShadowOp` changes in a breaking way and add
/// migration/dispatch logic to `decode_entry`. Mirrors `ddl_op_log.rs`'s
/// `DDL_OP_LOG_VERSION` pattern.
///
/// This log is crash-recovery-critical (migration replay reads it after
/// a crash to reconstruct in-flight writes) — an unrecognized version
/// byte MUST fail closed with a coded error rather than risk silently
/// misparsing recovery state.
const SHADOW_LOG_VERSION: u8 = 0x01;

/// Encode a `ShadowEntry` as `[VERSION_BYTE | bincode(entry)]`.
fn encode_entry(entry: &ShadowEntry) -> DbResult<Bytes> {
    let body =
        bincode::serialize(entry).map_err(|e| DbError::Codec(format!("shadow_log encode: {e}")))?;
    let mut bytes = Vec::with_capacity(1 + body.len());
    bytes.push(SHADOW_LOG_VERSION);
    bytes.extend_from_slice(&body);
    Ok(Bytes::from(bytes))
}

/// Decode a `[VERSION_BYTE | bincode(entry)]` blob. Fails closed (coded
/// `DbError::Codec`, no panic, no silent misparse as the current shape)
/// on an empty value or an unrecognized version byte.
fn decode_entry(bytes: &[u8]) -> DbResult<ShadowEntry> {
    let Some((&version, body)) = bytes.split_first() else {
        return Err(DbError::Codec(
            "shadow_log decode: empty entry (missing version byte)".to_string(),
        ));
    };
    if version != SHADOW_LOG_VERSION {
        return Err(DbError::Codec(format!(
            "shadow_log decode: unrecognized version {version:#04x} (expected {SHADOW_LOG_VERSION:#04x})"
        )));
    }
    bincode::deserialize(body).map_err(|e| DbError::Codec(format!("shadow_log decode: {e}")))
}

/// Max entries a single `read_from` call returns. Keys are BE-lsn
/// suffixed under a fixed migration prefix, so a single page never
/// spans two migrations. The caller (drain loop) re-invokes `read_from`
/// to page through a larger backlog rather than buffering it all in
/// one call — bounds memory and pairs with
/// `MigrationCoordinator::DRAIN_PASS_CAP` to guarantee
/// `drain_until_caught_up` terminates under sustained writes.
pub(crate) const READ_FROM_PAGE_CAP: usize = 4 * MAINT_SCAN_BATCH;

pub struct MigrationShadowLog {
    migration_id: String,
    store: Arc<dyn Store>,
    next_lsn: AtomicU64,
}

impl MigrationShadowLog {
    pub fn new(migration_id: String, store: Arc<dyn Store>) -> Self {
        Self {
            migration_id,
            store,
            next_lsn: AtomicU64::new(1),
        }
    }

    pub async fn recover(migration_id: String, store: Arc<dyn Store>) -> DbResult<Self> {
        let prefix = Self::key_prefix_static(&migration_id);
        let mut max_lsn = 0u64;
        let mut stream = store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                if let Some(lsn) = Self::parse_lsn_from_key(key.as_ref()) {
                    if lsn > max_lsn {
                        max_lsn = lsn;
                    }
                }
            }
        }
        Ok(Self {
            migration_id,
            store,
            next_lsn: AtomicU64::new(max_lsn + 1),
        })
    }

    pub fn current_lsn(&self) -> u64 {
        self.next_lsn.load(Ordering::Relaxed).saturating_sub(1)
    }

    pub fn next_lsn(&self) -> u64 {
        self.next_lsn.load(Ordering::Relaxed)
    }

    pub async fn append(&self, op: ShadowOp) -> DbResult<u64> {
        let lsn = self.next_lsn.fetch_add(1, Ordering::Relaxed);
        let entry = ShadowEntry { lsn, op };
        let key = self.entry_key(lsn);
        let value = encode_entry(&entry)?;
        self.store.set(key, value).await?;
        Ok(lsn)
    }

    pub async fn append_batch(&self, ops: Vec<ShadowOp>) -> DbResult<Vec<u64>> {
        if ops.is_empty() {
            return Ok(vec![]);
        }
        let base_lsn = self.next_lsn.fetch_add(ops.len() as u64, Ordering::Relaxed);
        let mut items = Vec::with_capacity(ops.len());
        let mut lsns = Vec::with_capacity(ops.len());
        for (i, op) in ops.into_iter().enumerate() {
            let lsn = base_lsn + i as u64;
            lsns.push(lsn);
            let entry = ShadowEntry { lsn, op };
            let key = self.entry_key(lsn);
            let value = encode_entry(&entry)?;
            items.push((key, value));
        }
        self.store.set_many(items).await?;
        Ok(lsns)
    }

    pub async fn read_from(&self, start_lsn: u64) -> DbResult<Vec<ShadowEntry>> {
        // Range-scan directly from `(id, start_lsn)` instead of rescanning
        // the whole `__shadow_<id>_` prefix from lsn 0 on every call. Keys
        // are BE-lsn-suffixed, so byte order == lsn order and the stream is
        // already sorted — no redundant re-sort. Bounded by
        // `READ_FROM_PAGE_CAP` so a large backlog isn't buffered unbounded
        // in one call.
        let start = ShadowKey::new(&self.migration_id, start_lsn).to_bytes();
        let end = ShadowKey::new(&self.migration_id, u64::MAX).to_bytes();
        let mut entries = Vec::new();
        let mut stream = self
            .store
            .iter_range_stream(Some(start), Some(end), MAINT_SCAN_BATCH);
        'page: while let Some(batch) = stream.next().await {
            for (_, value) in batch? {
                let entry = decode_entry(&value)?;
                entries.push(entry);
                if entries.len() >= READ_FROM_PAGE_CAP {
                    break 'page;
                }
            }
        }
        Ok(entries)
    }

    pub async fn purge(&self) -> DbResult<u64> {
        let prefix = self.key_prefix();
        let mut keys = Vec::new();
        let mut stream = self.store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        while let Some(batch) = stream.next().await {
            for (key, _) in batch? {
                keys.push(key);
            }
        }
        let count = keys.len() as u64;
        if !keys.is_empty() {
            self.store.remove_many(keys).await?;
        }
        Ok(count)
    }

    fn key_prefix(&self) -> Bytes {
        ShadowKey::scan_prefix(&self.migration_id)
    }

    fn key_prefix_static(migration_id: &str) -> Bytes {
        ShadowKey::scan_prefix(migration_id)
    }

    fn entry_key(&self, lsn: u64) -> shamir_storage::types::RecordKey {
        ShadowKey::new(&self.migration_id, lsn).to_record_key()
    }

    fn parse_lsn_from_key(key: &[u8]) -> Option<u64> {
        ShadowKey::parse_lsn(key)
    }
}
