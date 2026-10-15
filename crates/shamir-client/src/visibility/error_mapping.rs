use shamir_query_types::wire::DbResponse;

use crate::ClientError;

/// Map only actual wire fields; generic Error carries no structured positions.
pub(crate) fn map_response(response: DbResponse) -> Result<DbResponse, ClientError> {
    match response {
        DbResponse::AsOfVisibilityTimeout {
            code,
            message,
            requested,
            visible_through,
            blocking_version,
        } => Err(ClientError::AsOfVisibilityTimeout {
            code,
            message,
            requested,
            visible_through,
            blocking_version,
        }),
        DbResponse::Error { code, message } => Err(match code.as_str() {
            "as_of_future_version" => ClientError::AsOfFutureVersion { message },
            "as_of_reopen_required" => ClientError::AsOfReopenRequired { message },
            "resync_required" => ClientError::ResyncRequired { message },
            "visibility_future_version" => ClientError::VisibilityFutureVersion { message },
            "visibility_wait_invalid" => ClientError::VisibilityWaitInvalid { message },
            "write_committed_visibility_pending" => {
                ClientError::WriteCommittedVisibilityPending { message }
            }
            _ => ClientError::Db { code, message },
        }),
        other => Ok(other),
    }
}
