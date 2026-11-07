use std::path::Path;

#[cfg(feature = "fjall")]
use shamir_engine::repo::BoxRepoFactory;
use shamir_engine::repo::{RepoFactory, SeedInstallState};
use shamir_storage::types::Repo;

use crate::{DbError, DbResult, ShamirDb};

impl ShamirDb {
    /// Inspect raw metadata before add_repo can recover or insert a handle.
    pub(crate) async fn seed_boot_gate(&self, engine: &str, path: Option<&str>) -> DbResult<()> {
        let designated = path.is_some_and(|p| Path::new(p).join(".seed-install").exists());
        #[cfg(feature = "fjall")]
        let factory = if engine == "fjall" || engine == "fjall_raw" {
            path.map(BoxRepoFactory::fjall_raw)
        } else {
            Self::factory_from_meta(engine, path)
        };
        #[cfg(not(feature = "fjall"))]
        let factory = Self::factory_from_meta(engine, path);
        let factory =
            factory.ok_or_else(|| DbError::Storage("unsupported repository engine".into()))?;
        let backing = factory.create().await?;
        let info = backing.store_get("__tx__").await?;
        SeedInstallState::require_complete(&info, designated).await?;
        Ok(())
    }
}
