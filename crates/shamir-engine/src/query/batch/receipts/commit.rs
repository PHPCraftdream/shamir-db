use crate::query::batch::BatchError;
use crate::repo::RepoInstance;
use crate::tx::CommitError;
use shamir_query_types::batch::{TransactionInfo, VisibilityWait};

/// Preserve known publication, including durability warnings, as metadata.
/// The commit itself is never raced against an execution deadline.
pub(crate) async fn commit_implicit(
    repo: &RepoInstance,
    tx: shamir_tx::TxContext,
    alias: &str,
    wait: VisibilityWait,
) -> Result<TransactionInfo, BatchError> {
    let tx_id = tx.tx_id.0;
    let snapshot = tx.snapshot_version;
    match repo.commit_tx_with_options(tx, wait).await {
        Ok(outcome) => Ok(TransactionInfo::committed(
            outcome.tx_id,
            outcome.snapshot_version,
            outcome.commit_version,
            outcome.materialized(),
        )
        .with_visibility(outcome.visibility)),
        Err(error @ CommitError::WalDurability { .. }) => {
            Ok(error.into_transaction_info(tx_id, snapshot))
        }
        Err(error) => {
            let code = match &error {
                CommitError::MigrationFrozen { .. } => Some("table_migrating"),
                CommitError::UniqueViolation { .. } => Some("unique_violation"),
                CommitError::CasConflict { .. } => Some("version_conflict"),
                CommitError::PhantomConflict { .. }
                | CommitError::SsiConflict { .. }
                | CommitError::Wounded { .. } => Some("tx_conflict"),
                CommitError::WalRequiresReopen => Some("wal_requires_reopen"),
                CommitError::Storage(_) | CommitError::Expired { .. } => None,
                CommitError::WalDurability { .. } => Some("wal_durability"),
            };
            Err(BatchError::QueryError {
                alias: alias.into(),
                message: error.to_string(),
                code: code.map(str::to_owned),
            })
        }
    }
}
