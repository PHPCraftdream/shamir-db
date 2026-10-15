use crate::repo::RepoWritePermit;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_membuffer::MemBufferConfig;

use super::buffer_config;
use super::table_manager::TableManager;

impl TableManager {
    /// Read the persisted buffer config, if any. Returns `None`
    /// when no DDL has set one — the store still uses whatever
    /// default the factory wrapped them with.
    pub async fn get_buffer_config(&self) -> DbResult<Option<MemBufferConfig>> {
        buffer_config::load(&self.info_store).await
    }

    /// Persist a buffer config and hot-apply it to both stores.
    /// Idempotent — replays cleanly across restarts because the
    /// persisted value is reloaded on `TableManager::create`.
    ///
    /// S5c-X: buffer-config persistence mutates the shared info-twin, so it
    /// pays repository admission like every other DDL/registry write
    /// (closing the last table-layer bypass).
    ///
    /// cancel-safe: NO — persist → apply on data store → apply on
    /// info store. Cancellation between persist and apply leaves
    /// the live store config out of sync with the persisted value
    /// until the next restart (idempotent reload converges). Do NOT
    /// call under `tokio::select!` / `tokio::time::timeout`.
    pub async fn set_buffer_config(&self, cfg: &MemBufferConfig) -> DbResult<()> {
        let permit = self.admit().await?;
        self.set_buffer_config_admitted(cfg, &permit).await
    }

    /// S5c-X admitted twin — caller already holds repo admission.
    pub(crate) async fn set_buffer_config_admitted(
        &self,
        cfg: &MemBufferConfig,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
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
    ///
    /// S5c-X: admits ONCE for the whole read-modify-write (the admitted
    /// twin keeps the persist+apply sequence inside the same critical
    /// section — no double admission).
    pub async fn alter_buffer_config<F>(&self, mutate: F) -> DbResult<MemBufferConfig>
    where
        F: FnOnce(&mut MemBufferConfig),
    {
        let permit = self.admit().await?;
        let mut cfg = self.get_buffer_config().await?.unwrap_or_default();
        mutate(&mut cfg);
        self.set_buffer_config_admitted(&cfg, &permit).await?;
        Ok(cfg)
    }
}
