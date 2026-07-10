//! Per-table buffer config persistence + load on open.
//!
//! `MemBufferConfig` knobs (max_bytes, max_entries, ttl_ms,
//! flush_interval_ms, flush_batch_size) are tunable per-table via
//! DDL. The value is persisted under one system record in
//! `info_store`; loaded on `TableManager::create`; applied
//! hot-reload-style to the store stack via
//! `Store::apply_buffer_config`.

use crate::meta::MetaKey;
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_membuffer::MemBufferConfig;
use shamir_storage::types::{RecordKey, Store};

/// System record-key used to persist the per-table buffer config.
/// Built alloc-free via [`RecordKey::from_slice`] — the 16-byte system
/// `RecordId` inlines (no heap `Bytes::copy_from_slice`).
fn buffer_config_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::BufferConfig.as_record_id().as_bytes())
}

/// Read the persisted buffer config for the table whose
/// `info_store` is given. Returns `None` if no DDL has set one
/// (caller should fall back to whatever default came from the
/// factory).
pub async fn load(info_store: &Arc<dyn Store>) -> DbResult<Option<MemBufferConfig>> {
    let key = buffer_config_key();
    match info_store.get(key).await {
        Ok(bytes) => {
            let cfg: MemBufferConfig = bincode::deserialize(&bytes)
                .map_err(|e| DbError::Codec(format!("buffer_config decode: {e}")))?;
            Ok(Some(cfg))
        }
        Err(DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Write a buffer config to the info_store. Does NOT itself apply
/// the config to any live store — the caller (TableManager) is
/// responsible for invoking `Store::apply_buffer_config` on data
/// and info stores if hot-reload is desired.
pub async fn save(info_store: &Arc<dyn Store>, cfg: &MemBufferConfig) -> DbResult<()> {
    let bytes = bincode::serialize(cfg)
        .map_err(|e| DbError::Codec(format!("buffer_config encode: {e}")))?;
    info_store
        .set(buffer_config_key(), Bytes::from(bytes))
        .await?;
    Ok(())
}
