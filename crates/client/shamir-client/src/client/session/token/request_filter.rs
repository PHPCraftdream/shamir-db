use shamir_query_types::wire::DbRequest;

use crate::error::ClientError;

pub(crate) fn check_request_allowed(req: &DbRequest) -> Result<(), ClientError> {
    match req {
        DbRequest::Ping
        | DbRequest::Execute { .. }
        | DbRequest::WaitForVisibility { .. }
        | DbRequest::GetDdlOpStatus { .. } => Ok(()),
        DbRequest::TxBegin { .. }
        | DbRequest::TxExecute { .. }
        | DbRequest::TxCommit { .. }
        | DbRequest::TxRollback { .. }
        | DbRequest::CreateCursor { .. }
        | DbRequest::FetchNext { .. }
        | DbRequest::CancelCursor { .. } => Err(ClientError::Db {
            code: "not_supported_over_rest".into(),
            message: "not supported over REST".into(),
        }),
        // Admin-shell PR3: operator status and exact-key point reads
        // are not token surfaces — explicit refusal, never a wildcard.
        DbRequest::CreateScramUser { .. }
        | DbRequest::SetSuperuser { .. }
        | DbRequest::SetReplicator { .. }
        | DbRequest::Repl(_)
        | DbRequest::ChangePasswordChallenge { .. }
        | DbRequest::ChangePasswordVerify(_)
        | DbRequest::CreateApiToken { .. }
        | DbRequest::ListApiTokens { .. }
        | DbRequest::RevokeApiToken { .. }
        | DbRequest::RevokeUserTokens { .. }
        | DbRequest::Status
        | DbRequest::GetRecord { .. } => Err(ClientError::Db {
            code: "token_request_denied".into(),
            message: "request not allowed for token clients".into(),
        }),
    }
}
