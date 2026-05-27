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
use shamir_storage::types::Store;

/// System record-id used to persist the per-table buffer config.
fn buffer_config_key() -> Bytes {
    MetaKey::BufferConfig.as_record_id().to_bytes()
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

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryRepo;
    use shamir_storage::types::Repo;

    fn cfg() -> MemBufferConfig {
        MemBufferConfig {
            max_bytes: 1024 * 1024,
            max_entries: 200,
            ttl_ms: Some(5_000),
            flush_interval_ms: 250,
            flush_batch_size: 64,
        }
    }

    #[tokio::test]
    async fn save_then_load_roundtrip() {
        let repo = InMemoryRepo::new();
        let info_store: Arc<dyn Store> = repo.store_get("info").await.unwrap();
        save(&info_store, &cfg()).await.unwrap();
        let loaded = load(&info_store).await.unwrap().expect("config present");
        assert_eq!(loaded.max_bytes, cfg().max_bytes);
        assert_eq!(loaded.max_entries, cfg().max_entries);
        assert_eq!(loaded.ttl_ms, cfg().ttl_ms);
        assert_eq!(loaded.flush_interval_ms, cfg().flush_interval_ms);
        assert_eq!(loaded.flush_batch_size, cfg().flush_batch_size);
    }

    #[tokio::test]
    async fn load_returns_none_when_absent() {
        let repo = InMemoryRepo::new();
        let info_store: Arc<dyn Store> = repo.store_get("info").await.unwrap();
        let loaded = load(&info_store).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn save_overwrites_previous_config() {
        let repo = InMemoryRepo::new();
        let info_store: Arc<dyn Store> = repo.store_get("info").await.unwrap();
        save(&info_store, &cfg()).await.unwrap();

        let mut updated = cfg();
        updated.max_bytes = 64 * 1024 * 1024;
        updated.ttl_ms = None;
        save(&info_store, &updated).await.unwrap();

        let loaded = load(&info_store).await.unwrap().unwrap();
        assert_eq!(loaded.max_bytes, 64 * 1024 * 1024);
        assert_eq!(loaded.ttl_ms, None);
    }
}
