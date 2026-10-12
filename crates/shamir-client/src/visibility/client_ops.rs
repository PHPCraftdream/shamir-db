use shamir_query_types::batch::{BatchRequest, BatchResponse, VisibilityState, VisibilityWait};
use shamir_query_types::wire::{DbRequest, CURRENT_QUERY_LANG_VERSION};

use super::CommitResult;
use crate::{Client, ClientError};

impl Client {
    /// Observe settlement, not the commit decision of an unknown write.
    /// Reconcile only against the same repository incarnation; then open a new snapshot.
    pub async fn wait_for_visibility(
        &self,
        db: &str,
        repo: &str,
        version: u64,
        wait: VisibilityWait,
    ) -> Result<VisibilityState, ClientError> {
        let request = DbRequest::WaitForVisibility {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.into(),
            repo: repo.into(),
            version,
            visibility_wait: wait,
        };
        super::observe_once(version, self.roundtrip(&request)).await
    }

    /// No replay or rollback on pending, durability warnings or unknown acknowledgements.
    /// cancel-safe: NO — cancellation after send leaves the commit acknowledgement unknown.
    pub async fn tx_commit(
        &self,
        db: &str,
        tx_handle: u64,
        visibility_wait: Option<VisibilityWait>,
    ) -> Result<CommitResult, ClientError> {
        let request = DbRequest::TxCommit {
            query_version: CURRENT_QUERY_LANG_VERSION,
            visibility_wait,
            db: db.into(),
            tx_handle,
        };
        super::commit_once(&request, self.roundtrip(&request)).await
    }

    /// Execute an explicit transactional batch without discarding its receipt or rows.
    pub async fn execute_transactional(
        &self,
        db: &str,
        mut batch: BatchRequest,
        visibility_wait: Option<VisibilityWait>,
    ) -> Result<(BatchResponse, CommitResult), ClientError> {
        batch.transactional = true;
        batch.visibility_wait = visibility_wait;
        let response = self.execute(db, batch).await?;
        let transaction = response
            .transaction
            .clone()
            .ok_or_else(|| ClientError::Protocol("missing transactional receipt".into()))?;
        let receipt = CommitResult::from_transaction(transaction)?;
        Ok((response, receipt))
    }
}
