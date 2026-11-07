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
        _ => Err(ClientError::Db {
            code: "token_request_denied".into(),
            message: "request not allowed for token clients".into(),
        }),
    }
}
