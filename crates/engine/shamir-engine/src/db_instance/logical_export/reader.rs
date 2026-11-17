use super::{LogicalRows, LogicalTable, MaintenanceGuard};
use crate::db_instance::db_instance::DbInstance;
use crate::repo::RepoInstance;
use crate::table::TableManager;
use shamir_storage::error::{DbError, DbResult};
use std::collections::BTreeMap;
use std::time::SystemTime;

/// Read session for one database plus its separately owned system repository.
/// All methods require the same borrowed freeze capability. Authorization,
/// scope/reference closure and authoritative server identities belong to the
/// caller; this trusted engine surface does not authorize actors.
///
/// Inventory is capped at 65,536 repositories/tables and 64 MiB of names.
/// Record cursors never materialize a table. Cursor order is NOT canonical;
/// the container writer must externally sort raw keys before publication.
pub struct LogicalExport<'a> {
    guard: &'a MaintenanceGuard,
    instant: SystemTime,
    repos: BTreeMap<String, (RepoInstance, shamir_tx::SnapshotGuard)>,
    system: (RepoInstance, shamir_tx::SnapshotGuard),
}

impl<'a> LogicalExport<'a> {
    /// Call only after every server mutation source has enrolled in the shared
    /// gate (or under exclusive offline ownership). Drain detached commit tails
    /// before recording T and pin every repository's contiguous visible floor.
    /// No WAL/history drainer is cancelled or reimplemented here.
    pub async fn capture(
        db: &DbInstance,
        system: &RepoInstance,
        guard: &'a MaintenanceGuard,
    ) -> DbResult<Self> {
        guard.check_deadline()?;
        if db.repo_count() > 65_536 || db.table_count() > 65_536 {
            return Err(DbError::Validation(
                "logical export inventory exceeds limit".into(),
            ));
        }
        let mut repos = BTreeMap::new();
        let mut name_bytes = 0usize;
        for name in db.list_repos() {
            name_bytes += name.len();
            let repo = db
                .get_repo(&name)
                .ok_or_else(|| DbError::NotFound(name.clone()))?;
            for table in repo.list_table_names() {
                name_bytes += table.len();
            }
            if name_bytes > 64 * 1024 * 1024 {
                return Err(DbError::Validation(
                    "logical export inventory exceeds byte limit".into(),
                ));
            }
            repo.quiesce_commits(std::time::Duration::from_secs(600))
                .await?;
            let gate = repo.tx_gate().await?;
            if gate.requires_reopen() {
                return Err(DbError::RepoRequiresReopen(name));
            }
            let snapshot = gate.open_snapshot().await;
            if snapshot.version() != gate.last_committed() {
                return Err(DbError::Conflict(
                    "unsettled repository versions prevent logical export".into(),
                ));
            }
            repos.insert(name, (repo, snapshot));
        }
        system
            .quiesce_commits(std::time::Duration::from_secs(600))
            .await?;
        let gate = system.tx_gate().await?;
        if gate.requires_reopen() {
            return Err(DbError::RepoRequiresReopen(system.name().into()));
        }
        let snapshot = gate.open_snapshot().await;
        if snapshot.version() != gate.last_committed() {
            return Err(DbError::Conflict(
                "unsettled system versions prevent logical export".into(),
            ));
        }
        guard.charge_metadata(name_bytes)?;
        guard.check_deadline()?;
        Ok(Self {
            guard,
            instant: SystemTime::now(),
            repos,
            system: (system.clone(), snapshot),
        })
    }

    pub fn instant(&self) -> SystemTime {
        self.instant
    }

    /// Bounded inventory iterator; no second inventory allocation.
    pub fn repository_versions(&self) -> impl Iterator<Item = (&str, u64)> {
        self.repos
            .iter()
            .map(|(name, (_, pin))| (name.as_str(), pin.version()))
    }

    pub fn system_version(&self) -> u64 {
        self.system.1.version()
    }

    pub fn table_names(&self, repo: &str) -> DbResult<Vec<String>> {
        self.guard.check_deadline()?;
        Ok(self.repository(repo)?.0.list_table_names())
    }

    /// Resolve a table for read-only metadata and bounded record paging.
    pub fn table(&self, repo: &str, table: &str) -> DbResult<LogicalTable<'_>> {
        self.guard.check_deadline()?;
        Ok(LogicalTable::new(
            initialized(&self.repository(repo)?.0, table)?,
            self.guard,
        ))
    }

    pub async fn records(&self, repo: &str, table: &str) -> DbResult<LogicalRows<'_>> {
        let (instance, pin) = self.repository(repo)?;
        let table = initialized(instance, table)?;
        LogicalRows::new(table, pin.version(), self.guard, false).await
    }

    /// Portable catalogue families only: never expose retired users/roles or
    /// replication configuration. Settings are further restricted in the cursor
    /// to root_meta/fn_namespace_meta; counters and secret settings stay private.
    /// Rows retain schemas, FunctionMeta, version/call_signature, wasm_b64,
    /// validator/folder definitions, groups and embedded ResourceMeta verbatim.
    fn catalogue(&self, family: &str) -> DbResult<TableManager> {
        self.guard.check_deadline()?;
        if !matches!(
            family,
            "databases"
                | "repositories"
                | "tables"
                | "settings"
                | "functions"
                | "validators"
                | "function_folders"
                | "groups"
        ) {
            return Err(DbError::Validation(
                "excluded logical catalogue family".into(),
            ));
        }
        initialized(&self.system.0, family)
    }

    pub async fn catalogue_rows(&self, family: &str) -> DbResult<LogicalRows<'_>> {
        let table = self.catalogue(family)?;
        LogicalRows::new(table, self.system.1.version(), self.guard, true).await
    }

    fn repository(&self, name: &str) -> DbResult<&(RepoInstance, shamir_tx::SnapshotGuard)> {
        self.repos
            .get(name)
            .ok_or_else(|| DbError::NotFound(format!("export repository '{name}'")))
    }
}

// Cold opens can run repairs. Prepare included tables BEFORE entering the gate;
// an export must never invoke lazy write-producing initialization under freeze.
fn initialized(repo: &RepoInstance, name: &str) -> DbResult<TableManager> {
    repo.core.catalog.initialized_table(name).ok_or_else(|| {
        DbError::Validation(format!(
            "table '{name}' must be opened before logical export"
        ))
    })
}
