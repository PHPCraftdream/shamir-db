use shamir_query_types::batch::{
    BatchRequest, BatchResponse, IsolationLevel, VisibilityState, VisibilityWait,
};
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

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
        crate::receipts::wait_for_visibility_via(
            |req| async move { self.roundtrip(&req).await },
            db,
            repo,
            version,
            wait,
        )
        .await
    }

    /// Open one interactive (multi-call) transaction on `repo`.
    ///
    /// The server parks the transaction against this session and mints the
    /// opaque handle returned here; it stays open until [`Self::tx_commit`]
    /// or [`Self::tx_rollback`], or the session ends.
    ///
    /// cancel-safe: NO — a cancelled call may leave a transaction open.
    pub async fn tx_begin(
        &self,
        db: &str,
        repo: &str,
        isolation: Option<IsolationLevel>,
    ) -> Result<u64, ClientError> {
        let request = DbRequest::TxBegin {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.into(),
            repo: repo.into(),
            isolation,
        };
        match self.roundtrip(&request).await? {
            DbResponse::TxOpened { tx_handle, .. } => Ok(tx_handle),
            other => Err(ClientError::Protocol(format!(
                "expected TxOpened, got {other:?}"
            ))),
        }
    }

    /// Run one batch inside the open interactive transaction `tx_handle`
    /// without committing it.
    ///
    /// cancel-safe: NO — a cancelled call may leave a statement in flight.
    pub async fn tx_execute(
        &self,
        db: &str,
        tx_handle: u64,
        batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        let request = DbRequest::TxExecute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.into(),
            tx_handle,
            batch,
        };
        match self.roundtrip(&request).await? {
            DbResponse::TxBatch { response } => Ok(response),
            other => Err(ClientError::Protocol(format!(
                "expected TxBatch, got {other:?}"
            ))),
        }
    }

    /// Roll back an open interactive transaction, dropping every staged write.
    ///
    /// cancel-safe: NO — a cancelled call may leave the outcome unknown.
    pub async fn tx_rollback(&self, db: &str, tx_handle: u64) -> Result<(), ClientError> {
        let request = DbRequest::TxRollback {
            db: db.into(),
            tx_handle,
        };
        match self.roundtrip(&request).await? {
            DbResponse::TxRolledBack {
                tx_handle: rolled_back,
            } if rolled_back == tx_handle => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected TxRolledBack({tx_handle}), got {other:?}"
            ))),
        }
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
