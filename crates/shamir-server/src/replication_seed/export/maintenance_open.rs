use std::fs::File;
use std::path::Path;

use shamir_db::{ShamirDb, SystemStoreConfig};

use crate::replication_seed::error::invalid;
use crate::replication_seed::SeedError;

/// Offline ownership through the server's database initialization/recovery path.
/// Repository-internal WAL, drainer and changefeed owners are intentional; no
/// listener, supervisor, scheduler, audit task or user connection is constructed.
/// Fields drop in declaration order: release the database before its OS lock.
pub(crate) struct MaintenanceOpen {
    pub db: ShamirDb,
    _lock: File,
}

impl MaintenanceOpen {
    pub async fn open(data_dir: &Path) -> Result<Self, SeedError> {
        let lock = crate::maintenance_lock::acquire(data_dir)?;
        let meta = data_dir.join("shamir_db_meta.redb");
        if !meta.is_dir() {
            return Err(invalid("source catalogue does not exist"));
        }
        // Same Fjall config and init implementation as ServerLauncher, with a
        // pre-materialization catalogue budget. No CREATE/DROP bootstrap ops.
        let db = ShamirDb::init_seed(SystemStoreConfig::Fjall(meta)).await?;
        crate::restore::consume_restore_marker(data_dir, &db)
            .await
            .map_err(|e| invalid(format!("restore history epoch: {e}")))?;
        Ok(Self { db, _lock: lock })
    }
}
