use std::future::Future;

use shamir_query_types::batch::VisibilityState;
use shamir_query_types::wire::DbResponse;

use crate::ClientError;

pub(crate) async fn observe_once<F>(version: u64, send: F) -> Result<VisibilityState, ClientError>
where
    F: Future<Output = Result<DbResponse, ClientError>>,
{
    match send.await? {
        DbResponse::VisibilityObserved {
            version: observed,
            visibility,
        } if observed == version => {
            visibility
                .validate()
                .map_err(|e| ClientError::Protocol(e.into()))?;
            Ok(visibility)
        }
        other => Err(ClientError::Protocol(format!(
            "expected VisibilityObserved, got {other:?}"
        ))),
    }
}
