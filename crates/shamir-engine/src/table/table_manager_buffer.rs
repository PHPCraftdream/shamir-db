use shamir_storage::error::DbResult;
use shamir_storage::storage_membuffer::MemBufferConfig;

use super::buffer_config;
use super::table_manager::TableManager;

impl TableManager {
    /// Read the persisted buffer config, if any. Returns `None`
    /// when no DDL has set one — the store still uses whatever
    /// default the factory wrapped it with.
    pub async fn get_buffer_config(&self) -> DbResult<Option<MemBufferConfig>> {
        buffer_config::load(&self.info_store).await
    }

    /// Persist a buffer config and hot-apply it to both stores.
    /// Idempotent — replays cleanly across restarts because the
    /// persisted value is reloaded on `TableManager::create`.
    ///
    /// cancel-safe: NO — persist → apply on data store → apply on
    /// info store. Cancellation between persist and apply leaves
    /// the live store config out of sync with the persisted value
    /// until the next restart (idempotent reload converges). Do NOT
    /// call under `tokio::select!` / `tokio::time::timeout`.
    pub async fn set_buffer_config(&self, cfg: &MemBufferConfig) -> DbResult<()> {
        buffer_config::save(&self.info_store, cfg).await?;
        self.table.data_store().apply_buffer_config(cfg).await?;
        self.info_store.apply_buffer_config(cfg).await?;
        Ok(())
    }

    /// Partial-update flavour: read the current persisted config
    /// (falling back to `MemBufferConfig::default()` if none was
    /// set), apply the closure to produce the new value, then
    /// persist and hot-apply. Convenient for "bump just one
    /// knob" DDL like `ALTER TABLE ... SET buffer.ttl_ms = ...`.
    pub async fn alter_buffer_config<F>(&self, mutate: F) -> DbResult<MemBufferConfig>
    where
        F: FnOnce(&mut MemBufferConfig),
    {
        let mut cfg = self.get_buffer_config().await?.unwrap_or_default();
        mutate(&mut cfg);
        self.set_buffer_config(&cfg).await?;
        Ok(cfg)
    }
}
