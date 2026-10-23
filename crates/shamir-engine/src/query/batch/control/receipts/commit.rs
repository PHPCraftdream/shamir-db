use std::future::Future;
use std::pin::Pin;

use crate::query::batch::BatchError;
use crate::repo::RepoInstance;
use crate::tx::ack::commit_tx_with_kind;
use crate::tx::CommitError;
use shamir_query_types::batch::{TransactionInfo, VisibilityWait};
use shamir_tx::visibility_metrics::PendingKind;

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

/// Wire `code` for a failed implicit-batch commit, surfaced through
/// `BatchError::QueryError` and passed verbatim to `DbResponse::Error` by the
/// server's `error_code` mapper.
pub(crate) fn receipt_error_code(error: &CommitError) -> Option<&'static str> {
    match error {
        CommitError::MigrationFrozen { .. } => Some("table_migrating"),
        CommitError::UniqueViolation { .. } => Some("unique_violation"),
        CommitError::CasConflict { .. } => Some("version_conflict"),
        CommitError::PhantomConflict { .. }
        | CommitError::SsiConflict { .. }
        | CommitError::Wounded { .. } => Some("tx_conflict"),
        CommitError::WalRequiresReopen => Some("wal_requires_reopen"),
        // S5c-X: commit admission refused while the retained sealed WAL exceeds
        // `durability.wal_retention_max_bytes` under mode `refuse_writes`.
        // RETRYABLE: the refusal clears by itself once the drainer truncates
        // the WAL back under the cap; a retried commit then succeeds.
        CommitError::WalRetentionExceeded => Some("wal_retention_exceeded"),
        CommitError::Storage(_) | CommitError::Expired { .. } => None,
        CommitError::WalDurability { .. } => Some("wal_durability"),
    }
}

async fn commit_implicit_inner(
    repo: &RepoInstance,
    tx: shamir_tx::TxContext,
    alias: &str,
    wait: VisibilityWait,
) -> Result<TransactionInfo, BatchError> {
    let tx_id = tx.tx_id.0;
    let snapshot = tx.snapshot_version;
    match commit_tx_with_kind(tx, repo, wait, PendingKind::ImplicitBatch).await {
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
            let code = receipt_error_code(&error);
            Err(BatchError::QueryError {
                alias: alias.into(),
                message: error.to_string(),
                code: code.map(str::to_owned),
            })
        }
    }
}
