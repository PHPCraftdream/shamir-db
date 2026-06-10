use bytes::Bytes;
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
        use futures::StreamExt;
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
        let value = bincode::serialize(&entry)
            .map_err(|e| DbError::Codec(format!("shadow_log encode: {e}")))?;
        self.store.set(key, Bytes::from(value)).await?;
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
            let value = bincode::serialize(&entry)
                .map_err(|e| DbError::Codec(format!("shadow_log encode: {e}")))?;
            items.push((key, Bytes::from(value)));
        }
        self.store.set_many(items).await?;
        Ok(lsns)
    }

    pub async fn read_from(&self, start_lsn: u64) -> DbResult<Vec<ShadowEntry>> {
        let prefix = self.key_prefix();
        let mut entries = Vec::new();
        let mut stream = self.store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        use futures::StreamExt;
        while let Some(batch) = stream.next().await {
            for (key, value) in batch? {
                if let Some(lsn) = Self::parse_lsn_from_key(key.as_ref()) {
                    if lsn >= start_lsn {
                        let entry: ShadowEntry = bincode::deserialize(&value)
                            .map_err(|e| DbError::Codec(format!("shadow_log decode: {e}")))?;
                        entries.push(entry);
                    }
                }
            }
        }
        entries.sort_by_key(|e| e.lsn);
        Ok(entries)
    }

    pub async fn purge(&self) -> DbResult<u64> {
        let prefix = self.key_prefix();
        let mut keys = Vec::new();
        let mut stream = self.store.scan_prefix_stream(prefix, MAINT_SCAN_BATCH);
        use futures::StreamExt;
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
