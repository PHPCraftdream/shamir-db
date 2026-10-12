use std::future::Future;

use shamir_query_types::wire::{DbRequest, DbResponse};

use super::CommitResult;
use crate::ClientError;

/// One attempt only: the transport owns deadlines; errors never trigger a replay.
pub(crate) async fn commit_once<F>(
    request: &DbRequest,
    send: F,
) -> Result<CommitResult, ClientError>
where
    F: Future<Output = Result<DbResponse, ClientError>>,
{
    if !matches!(request, DbRequest::TxCommit { .. }) {
        return Err(ClientError::Protocol("expected commit request".into()));
    }
    match send.await? {
        DbResponse::TxCommitted { transaction } => CommitResult::from_transaction(transaction),
        other => Err(ClientError::Protocol(format!(
            "expected TxCommitted, got {other:?}"
        ))),
    }
}
