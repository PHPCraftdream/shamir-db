use crate::meta::repo_identity::RepoIdentity;
use crate::repo::changelog_store::write_domain::RepoWritePermit;
use crate::table::table_manager::table_token_for;
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Repo;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
#[cfg(feature = "test-util")]
use tokio::sync::OnceCell;

use self::rename_attempt::RenameAttempt;
use self::rename_freeze::RenameFreeze;
pub use self::rename_outcome::RenamePersistOutcome;
use super::RepoInstance;

mod rename_attempt;
mod rename_fence;
mod rename_freeze;
pub(super) mod rename_in_doubt;
mod rename_outcome;
mod repo_membership_impl;
pub(crate) mod visibility_impl;

impl RepoInstance {
    /// Create a non-unique index on a table.
    pub async fn create_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        self.get_table(table_name)
            .await?
            .create_index(index_name, paths)
            .await
    }

    /// Create a unique index on a table.
    pub async fn create_unique_index(
        &self,
        table_name: &str,
        index_name: &str,
        paths: &[&str],
    ) -> DbResult<()> {
        self.get_table(table_name)
            .await?
            .create_unique_index(index_name, paths)
            .await
    }

    /// Drop a non-unique index, returning whether it existed.
    pub async fn drop_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        self.get_table(table_name)
            .await?
            .drop_index(index_name, None)
            .await
    }

    /// Drop a unique index, returning whether it existed.
    pub async fn drop_unique_index(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        self.get_table(table_name)
            .await?
            .drop_unique_index(index_name, None)
            .await
    }

    /// Check whether a non-unique index exists.
    pub async fn index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        Ok(self
            .get_table(table_name)
            .await?
            .index_exists(index_name)
            .await)
    }

    /// Check whether a unique index exists.
    pub async fn unique_index_exists(&self, table_name: &str, index_name: &str) -> DbResult<bool> {
        Ok(self
            .get_table(table_name)
            .await?
            .unique_index_exists(index_name)
            .await)
    }

    /// Find record ids matching indexed values.
    pub async fn lookup_by_index(
        &self,
        table_name: &str,
        index_name: &str,
        values: &[InnerValue],
    ) -> DbResult<BTreeSet<RecordId>> {
        self.get_table(table_name)
            .await?
            .lookup_by_index(index_name, values)
            .await
    }

    /// Resolve a registered table token, initializing its table manager if needed.
    ///
    /// This lookup is intentionally unfenced: callers such as transaction prelock
    /// must reach the manager to observe a rename freeze, while fencing this lookup
    /// would make the drainer wait on work that cannot discover the freeze.
    pub async fn table_by_token(&self, token: u64) -> DbResult<Option<TableManager>> {
        match self.token_names.read_sync(&token, |_, name| name.clone()) {
            Some(name) => self.get_table_unfenced(&name).await.map(Some),
            None => Ok(None),
        }
    }

    /// Resolve a token only when its manager is already initialized.
    ///
    /// This is a non-instantiating peek: a dormant manager stays dormant, so keep
    /// the lookup non-instantiating if it is used by background/drain paths.
    pub(crate) async fn table_by_token_if_live(&self, token: u64) -> Option<TableManager> {
        let name = self.token_names.read_sync(&token, |_, name| name.clone())?;
        let cell = self
            .tables
            .get(&name)
            .map(|entry| Arc::clone(entry.value()))?;
        cell.get().cloned()
    }

    /// Install a prebuilt table manager for test fixtures.
    #[cfg(feature = "test-util")]
    pub fn install_table_for_test(&self, name: &str, table: TableManager) {
        self.tables
            .insert(name.into(), Arc::new(OnceCell::new_with(Some(table))));
    }

    /// List configured table names.
    pub fn list_table_names(&self) -> Vec<String> {
        self.configs
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Return whether a table is configured.
    pub fn has_table(&self, name: &str) -> bool {
        self.configs.contains_key(name)
    }

    /// Clone a table's configuration, if configured.
    pub fn table_config(&self, name: &str) -> Option<TableConfig> {
        self.configs.get(name).map(|entry| entry.value().clone())
    }

    /// Return mapped data, info and history store names for a table.
    pub(crate) fn mapped_table_store_names(&self, name: &str) -> Option<(String, String, String)> {
        let config = self.configs.get(name).map(|entry| entry.value().clone())?;
        let names = super::table_store::table_store_names(name, config.store_id.as_ref());
        Some((names.data, names.info, names.history))
    }

    /// Count configured tables.
    pub fn table_count(&self) -> usize {
        self.configs.len()
    }

    /// Count degraded indexes in initialized table managers.
    pub async fn degraded_index_count(&self) -> u64 {
        let tables: Vec<TableManager> = self
            .tables
            .iter()
            .filter_map(|entry| entry.value().get().cloned())
            .collect();
        let mut count = 0;
        for table in tables {
            count += table.degraded_index_count().await;
        }
        count
    }

    /// Rename table stores using the repository's normal write admission.
    ///
    /// The copy receives a fresh store id; the old stores remain as orphans and
    /// the old manager is retired. This path refuses a fenced rename pair when it
    /// cannot durably resolve the catalogue state; it must not guess at an unknown
    /// persistence outcome.
    pub async fn rename_table_stores(&self, from: &str, to: &str) -> DbResult<bool> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.rename_table_stores_admitted(from, to, &permit).await
    }

    /// Load or create the persistent repository identity.
    pub async fn repo_identity(&self) -> DbResult<RepoIdentity> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.repo_identity_admitted(&permit).await
    }

    /// Load or create the identity while holding repository write admission.
    pub(crate) async fn repo_identity_admitted(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<RepoIdentity> {
        self.write_domain.debug_check_permit(permit);
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        RepoIdentity::load_or_create(&store).await
    }

    /// Advance the repository history epoch.
    pub async fn bump_history_epoch(&self) -> DbResult<RepoIdentity> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.bump_history_epoch_admitted(&permit).await
    }

    /// Advance the history epoch while holding repository write admission.
    pub(crate) async fn bump_history_epoch_admitted(
        &self,
        permit: &RepoWritePermit,
    ) -> DbResult<RepoIdentity> {
        self.write_domain.debug_check_permit(permit);
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        RepoIdentity::bump(&store, None).await
    }

    /// Apply an idempotent restore operation to the history epoch.
    pub async fn apply_restore_operation(&self, operation: [u8; 16]) -> DbResult<RepoIdentity> {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.apply_restore_operation_admitted(operation, &permit)
            .await
    }

    /// Apply a restore operation while holding repository write admission.
    pub(crate) async fn apply_restore_operation_admitted(
        &self,
        operation: [u8; 16],
        permit: &RepoWritePermit,
    ) -> DbResult<RepoIdentity> {
        self.write_domain.debug_check_permit(permit);
        let _guard = self.marker_write_mutex.lock().await;
        let store = self.tx_info_store().await?;
        RepoIdentity::bump(&store, Some(operation)).await
    }

    /// Rename table stores after durable catalogue persistence.
    ///
    /// `persist` reports whether the catalogue commit is known, known not to have
    /// happened, or uncertain. Committed retires the source and completes the
    /// rename. A fresh NotCommitted attempt aborts its freeze and returns the
    /// original error. InDoubt (and NotCommitted while resuming a fenced attempt)
    /// retains the freeze and both names, returning a conflict so callers cannot
    /// mistake uncertainty for rollback. Cancellation before persistence aborts
    /// a fresh attempt; once persistence is underway, or for an already-fenced
    /// attempt, the guard is retained for recovery.
    pub async fn rename_table_stores_with<F, Fut>(
        &self,
        from: &str,
        to: &str,
        persist: F,
    ) -> DbResult<bool>
    where
        F: FnOnce(TableConfig) -> Fut,
        Fut: Future<Output = RenamePersistOutcome>,
    {
        self.rename_table_transition(from, to, persist).await
    }

    /// Reconcile an already-recorded table rename.
    ///
    /// A non-committed or uncertain persistence result leaves the source frozen
    /// and retains cancellation protection. Retrying the same name pair reuses
    /// its guard; recovery does not perform an extra drain or store copy.
    pub async fn recover_table_rename_with<F, Fut>(
        &self,
        from: &str,
        to: &str,
        config: TableConfig,
        expected_source_id: Option<&RecordId>,
        persist: F,
    ) -> DbResult<bool>
    where
        F: FnOnce(TableConfig) -> Fut,
        Fut: Future<Output = RenamePersistOutcome>,
    {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.check_rename_pair(from, to)?;
        if config.name != to || from == to {
            return Err(DbError::Validation(
                "rename recovery identity mismatch".into(),
            ));
        }
        let source = self.configs.get(from).map(|source| source.value().clone());
        let destination = self
            .configs
            .get(to)
            .map(|destination| destination.value().clone());
        let source_matches = source
            .as_ref()
            .is_some_and(|source| source.store_id.as_ref() == expected_source_id);
        if source.is_some() && !source_matches {
            return Err(DbError::Validation(
                "rename source incarnation changed".into(),
            ));
        }
        if destination.as_ref().is_some_and(|destination| {
            destination.store_id != config.store_id
                || destination.enable_indexes != config.enable_indexes
        }) {
            return Err(DbError::Validation(
                "rename destination incarnation changed".into(),
            ));
        }
        if !source_matches {
            // When the source registration is absent and only the destination
            // exists, the persist runs unfenced — there is no source manager or
            // freeze to hold; the returned conflict is advisory, not a
            // retryable fence (M7-style metric stale handles unchanged).
            if source.is_none() && destination.is_some() {
                return match persist(config.clone()).await {
                    RenamePersistOutcome::Committed => Ok(false),
                    RenamePersistOutcome::NotCommitted(error)
                    | RenamePersistOutcome::InDoubt(error) => {
                        Err(rename_in_doubt_error(from, to, error))
                    }
                };
            }
            return Err(DbError::Validation(
                "rename source registration is missing".into(),
            ));
        }
        let pair = self.take_rename_fence(from, to);
        let source_table = match pair.as_ref() {
            Some(pair) => pair.freeze.source().clone(),
            None => self.get_table_admitted(from, &permit).await?,
        };
        let mut attempt = RenameAttempt::new(self, &permit, from, to, source_table, pair);
        if !attempt.was_fenced() {
            match RenameFreeze::begin(attempt.source().clone(), &permit).await {
                Ok(freeze) => attempt.install_freeze(freeze),
                Err(error) => return Err(error),
            }
        }
        attempt.mark_persisting();
        match persist(config.clone()).await {
            RenamePersistOutcome::Committed => {
                self.remove_table(from, &permit);
                let registered = if destination.is_none() {
                    self.add_table(config, &permit);
                    true
                } else {
                    false
                };
                attempt.commit();
                Ok(registered)
            }
            RenamePersistOutcome::NotCommitted(error) | RenamePersistOutcome::InDoubt(error) => {
                attempt.into_doubt();
                Err(rename_in_doubt_error(from, to, error))
            }
        }
    }

    async fn rename_table_transition<F, Fut>(
        &self,
        from: &str,
        to: &str,
        persist: F,
    ) -> DbResult<bool>
    where
        F: FnOnce(TableConfig) -> Fut,
        Fut: Future<Output = RenamePersistOutcome>,
    {
        let permit = self
            .write_domain
            .admit()
            .await
            .map_err(|error| DbError::Internal(error.to_string()))?;
        self.check_rename_pair(from, to)?;
        if !self.configs.contains_key(from) {
            return Ok(false);
        }
        if self.configs.contains_key(to) {
            return Err(DbError::Validation(format!("Table '{to}' already exists")));
        }
        let pair = self.take_rename_fence(from, to);
        let source = match pair.as_ref() {
            Some(pair) => pair.freeze.source().clone(),
            None => self.get_table_admitted(from, &permit).await?,
        };
        let mut attempt = RenameAttempt::new(self, &permit, from, to, source, pair);
        if !attempt.was_fenced() {
            match RenameFreeze::begin(attempt.source().clone(), &permit).await {
                Ok(freeze) => attempt.install_freeze(freeze),
                Err(error) => return Err(error),
            }
        }
        let fenced = attempt.was_fenced();
        if let Err(error) = attempt.freeze().await_visible(self, from).await {
            attempt.abort();
            return Err(if fenced {
                rename_in_doubt_error(from, to, error)
            } else {
                error
            });
        }
        let config = match self
            .prepare_table_rename_admitted(from, to, &permit, attempt.freeze())
            .await
        {
            Ok(Some(config)) => config,
            Ok(None) => {
                attempt.abort();
                return Ok(false);
            }
            Err(error) => {
                attempt.abort();
                return Err(if fenced {
                    rename_in_doubt_error(from, to, error)
                } else {
                    error
                });
            }
        };
        attempt.mark_persisting();
        match persist(config.clone()).await {
            RenamePersistOutcome::Committed => {
                self.commit_table_rename_admitted(from, config, &permit)
                    .await?;

                attempt.commit();
                Ok(true)
            }
            RenamePersistOutcome::NotCommitted(error) if !fenced => {
                attempt.abort();
                Err(error)
            }
            RenamePersistOutcome::NotCommitted(error) | RenamePersistOutcome::InDoubt(error) => {
                attempt.into_doubt();
                Err(rename_in_doubt_error(from, to, error))
            }
        }
    }

    /// Rename while holding this repository's write-admission permit.
    ///
    /// Copies into stores with a fresh id, leaves the old stores orphaned, and
    /// retires the old manager after catalogue registration. A fenced pair is
    /// refused because this method has no catalogue persistence callback with
    /// which to resolve an unknown outcome.
    pub async fn rename_table_stores_admitted(
        &self,
        from: &str,
        to: &str,
        permit: &RepoWritePermit,
    ) -> DbResult<bool> {
        self.write_domain.debug_check_permit(permit);
        self.check_rename_pair(from, to)?;
        if !self.configs.contains_key(from) {
            return Ok(false);
        }
        if self.configs.contains_key(to) {
            return Err(DbError::Validation(format!("Table '{to}' already exists")));
        }
        self.check_table_rename_fence(from)?;
        self.check_table_rename_fence(to)?;
        let source = self.get_table_admitted_unfenced(from, permit).await?;
        let freeze = RenameFreeze::begin(source, permit).await?;
        freeze.await_visible(self, from).await?;
        let config = match self
            .prepare_table_rename_admitted(from, to, permit, &freeze)
            .await
        {
            Ok(Some(config)) => config,
            Ok(None) => {
                return Ok(false);
            }
            Err(error) => {
                return Err(error);
            }
        };
        self.commit_table_rename_admitted(from, config, permit)
            .await?;

        freeze.retire();
        Ok(true)
    }

    /// Prepare the copied stores and destination config while holding admission.
    ///
    /// The caller must first make the freeze visible to existing lookups. This
    /// drains committed MVCC overlays into history before physical store copy,
    /// then persists the source counter before copying data, info, and history.
    /// The source must remain frozen throughout. The MVCC `Arc` is cloned from
    /// the SCC bucket before awaiting, so no bucket guard is held across async work.
    pub(crate) async fn prepare_table_rename_admitted(
        &self,
        from: &str,
        to: &str,
        permit: &RepoWritePermit,
        freeze: &RenameFreeze,
    ) -> DbResult<Option<TableConfig>> {
        self.write_domain.debug_check_permit(permit);
        let Some(config) = self.configs.get(from).map(|entry| entry.value().clone()) else {
            return Ok(None);
        };
        if self.configs.contains_key(to) {
            return Err(DbError::Validation(format!("Table '{to}' already exists")));
        }
        // Committed MVCC overlay entries must reach history before physical
        // store copy, or rows not yet drained by the background task are missed.
        let from_token = table_token_for(from);
        // Clone the Arc while inside the synchronous SCC read closure; that lock
        // is released before awaiting the potentially blocking drain operation.
        if let Some(mvcc) = self
            .per_table_mvcc
            .read_sync(&from_token, |_, mvcc| Arc::clone(mvcc))
        {
            mvcc.drain_to_history().await?;
        }
        freeze.source().counter().persist_admitted(permit).await?;
        let config = self.copy_table_stores_for_rename(from, to, config).await?;
        Ok(Some(config))
    }

    async fn copy_table_stores_for_rename(
        &self,
        from: &str,
        to: &str,
        old_config: TableConfig,
    ) -> DbResult<TableConfig> {
        let source_names =
            super::table_store::table_store_names(from, old_config.store_id.as_ref());
        let fresh_id = super::table_store::new_table_store_id();
        let new_config = TableConfig {
            name: to.into(),
            enable_indexes: old_config.enable_indexes,
            store_id: Some(fresh_id),
        };
        new_config.validate_store_name()?;
        let destination_names =
            super::table_store::table_store_names(to, new_config.store_id.as_ref());
        let existing = self.repo.stores_list().await?;
        if existing.iter().any(|name| {
            [
                destination_names.data.as_str(),
                destination_names.info.as_str(),
                destination_names.history.as_str(),
            ]
            .contains(&name.as_str())
        }) {
            return Err(DbError::Validation(format!(
                "rename destination storage for '{to}' is already present"
            )));
        }
        super::table_store::copy_table_store(
            &self.repo,
            &source_names.data,
            &destination_names.data,
        )
        .await?;
        super::table_store::copy_table_store(
            &self.repo,
            &source_names.info,
            &destination_names.info,
        )
        .await?;
        super::table_store::copy_table_store(
            &self.repo,
            &source_names.history,
            &destination_names.history,
        )
        .await?;
        Ok(new_config)
    }

    async fn commit_table_rename_admitted(
        &self,
        from: &str,
        config: TableConfig,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let to = config.name.clone();
        if to == from {
            return Err(DbError::Validation(
                "rename source and destination must differ".into(),
            ));
        }
        if self.configs.contains_key(&to) {
            return Err(DbError::Validation(format!("Table '{to}' already exists")));
        }
        if !self.configs.contains_key(from) {
            return Err(DbError::NotFound(format!("Table '{from}' not found")));
        }
        self.remove_table(from, permit);
        self.add_table(config, permit);
        Ok(())
    }
}

fn rename_in_doubt_error(from: &str, to: &str, error: DbError) -> DbError {
    DbError::Conflict(format!("rename in doubt: '{from}' to '{to}': {error}"))
}
