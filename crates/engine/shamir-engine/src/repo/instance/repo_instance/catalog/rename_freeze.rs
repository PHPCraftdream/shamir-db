use crate::repo::RepoInstance;
use crate::repo::RepoWritePermit;
use crate::table::{MigrationFreezeError, MigrationFreezeGuard, TableManager};
use crate::tx::CommitError;
use shamir_storage::error::{DbError, DbResult};
use shamir_tx::repo_tx_gate::VisibleOutcome;
use std::time::Duration;

#[cfg(not(test))]
const RENAME_VISIBILITY_WAIT: Duration = Duration::from_secs(30);
#[cfg(test)]
const RENAME_VISIBILITY_WAIT: Duration = Duration::from_millis(200);

/// Owns the source table's freeze and detach state for one rename attempt.
/// The caller declares this after (or borrows) repository admission, so it
/// drops first on return or cancellation and can safely restore attachment.
pub(crate) struct RenameFreeze {
    source: TableManager,
    freeze: Option<MigrationFreezeGuard>,
}

impl RenameFreeze {
    /// Acquire the source freeze under the caller-held repository permit,
    /// then mark its manager detached for the duration of this attempt.
    pub(crate) async fn begin(source: TableManager, permit: &RepoWritePermit) -> DbResult<Self> {
        let freeze = source
            .begin_migration_freeze_admitted_impl(permit)
            .await
            .map_err(|error| match error {
                MigrationFreezeError::Storage(error) => error,
                MigrationFreezeError::MigrationFrozen { table } => {
                    DbError::MigrationFrozen { table }
                }
            })?;
        source.detach_for_rename();
        Ok(Self {
            source,
            freeze: Some(freeze),
        })
    }

    /// Wait for visibility of every version allocated before this call's
    /// high-water sample, bounded so a stalled settlement is retryable.
    pub(crate) async fn await_visible(&self, repo: &RepoInstance, from: &str) -> DbResult<()> {
        let wait = tokio::time::timeout(RENAME_VISIBILITY_WAIT, async {
            let gate = repo.tx_gate().await?;
            let target = gate.version_allocation_high_water_mark();
            Ok::<_, DbError>(gate.await_visible(target).await)
        })
        .await;
        match wait {
            Ok(Ok(VisibleOutcome::Visible)) => Ok(()),
            Ok(Ok(VisibleOutcome::ReopenRequired)) => Err(DbError::RepoRequiresReopen(format!(
                "rename of '{from}' requires repository reopen"
            ))),
            Ok(Err(error)) => Err(error),
            Err(_elapsed) => Err(DbError::Conflict(format!(
                "rename visibility wait timed out for '{from}'"
            ))),
        }
    }

    /// Return the frozen source manager for the counter flush during prepare.
    pub(crate) fn source(&self) -> &TableManager {
        &self.source
    }

    /// Retain detached/frozen state after successful registration cutover.
    pub(crate) fn retire(mut self) {
        if let Some(freeze) = self.freeze.take() {
            freeze.retire();
        }
    }
}

impl Drop for RenameFreeze {
    fn drop(&mut self) {
        if let Some(freeze) = self.freeze.take() {
            self.source.reattach_after_rename();
            drop(freeze);
        }
    }
}

/// Facade twin of the table-internal freeze entry points: maps the
/// table-local `MigrationFreezeError` into the public `CommitError`
/// contract for shamir-db.
impl TableManager {
    pub async fn begin_migration_freeze(&self) -> Result<MigrationFreezeGuard, CommitError> {
        self.begin_migration_freeze_impl()
            .await
            .map_err(|error| match error {
                MigrationFreezeError::Storage(error) => CommitError::Storage(error),
                MigrationFreezeError::MigrationFrozen { table } => {
                    CommitError::MigrationFrozen { table }
                }
            })
    }

    pub async fn begin_migration_freeze_admitted(
        &self,
        permit: &RepoWritePermit,
    ) -> Result<MigrationFreezeGuard, CommitError> {
        self.begin_migration_freeze_admitted_impl(permit)
            .await
            .map_err(|error| match error {
                MigrationFreezeError::Storage(error) => CommitError::Storage(error),
                MigrationFreezeError::MigrationFrozen { table } => {
                    CommitError::MigrationFrozen { table }
                }
            })
    }
}
