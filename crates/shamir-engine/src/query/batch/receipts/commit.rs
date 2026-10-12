use std::future::Future;
use std::pin::Pin;

use crate::query::batch::BatchError;
use crate::repo::RepoInstance;
use crate::tx::CommitError;
use shamir_query_types::batch::{TransactionInfo, VisibilityWait};

/// Preserve known publication, including durability warnings, as metadata.
/// The commit itself is never raced against an execution deadline.
///
/// Boxed on purpose: the commit pipeline future is large and this is awaited
/// from four write arms of the per-op runner, which recurses through nested
/// batches; inlining it would multiply the runner's stack footprint.
pub(crate) fn commit_implicit<'a>(
    repo: &'a RepoInstance,
    tx: shamir_tx::TxContext,
    alias: &'a str,
    wait: VisibilityWait,
) -> Pin<Box<dyn Future<Output = Result<TransactionInfo, BatchError>> + Send + 'a>> {
    Box::pin(commit_implicit_inner(repo, tx, alias, wait))
}

async fn commit_implicit_inner(
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
