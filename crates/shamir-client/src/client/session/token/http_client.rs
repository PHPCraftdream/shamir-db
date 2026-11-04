use shamir_query_types::batch::{BatchRequest, BatchResponse, VisibilityState, VisibilityWait};
use shamir_query_types::read::DdlOpStatus;
use shamir_query_types::wire::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
use zeroize::Zeroizing;

use super::options::TokenHttpOptions;
use super::request_filter::check_request_allowed;
use crate::client::transport::token::{map_ping_reply, map_reply, TokenTransport};
use crate::error::ClientError;

/// Token-authenticated client over the REST endpoint. Supports only
/// `ping`, `execute`, `wait_for_visibility` and `get_ddl_op_status`.
pub struct TokenHttpClient {
    transport: TokenTransport,
}

impl TokenHttpClient {
    /// Dials the endpoint. The token is moved into a sensitive header and the
    /// `ApiToken` is zeroized immediately.
    pub async fn connect(opts: TokenHttpOptions) -> Result<Self, ClientError> {
        let header = opts.token.authorization_header()?;
        let transport = TokenTransport::connect(
            &opts.endpoint,
            header,
            opts.connect_timeout,
            opts.request_timeout,
        )
        .await?;
        Ok(Self { transport })
    }

    /// cancel-safe: NO — a cancelled call may leave a request in flight.
    pub async fn ping(&self) -> Result<(), ClientError> {
        map_ping_reply(self.transport.get_ping().await?)
    }

    /// Sends one request; requests outside the token allowlist are refused
    /// locally and nothing is sent.
    ///
    /// The encoded body handed to the transport cannot be wiped afterwards.
    /// cancel-safe: NO — a cancelled call may leave a request in flight.
    pub async fn request(&self, req: &DbRequest) -> Result<DbResponse, ClientError> {
        check_request_allowed(req)?;
        let mut body = Zeroizing::new(Vec::<u8>::new());
        rmp_serde::encode::write_named(&mut *body, req).map_err(ClientError::Encode)?;
        let reply = self
            .transport
            .post_request(std::mem::take(&mut *body))
            .await?;
        crate::visibility::map_response(map_reply(reply)?)
    }

    /// Executes a batch. Unlike `Client::execute`, no interner epochs are
    /// announced and no interner delta is merged into a cache.
    ///
    /// cancel-safe: NO — a cancelled call may leave a request in flight.
    pub async fn execute(
        &self,
        db: &str,
        batch: BatchRequest,
    ) -> Result<BatchResponse, ClientError> {
        let req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: db.to_string(),
            batch,
        };
        match self.request(&req).await? {
            DbResponse::Batch { response } => Ok(response),
            other => Err(ClientError::Protocol(format!(
                "expected Batch, got {other:?}"
            ))),
        }
    }

    /// Observes settlement of `version` in `repo`.
    ///
    /// cancel-safe: NO — a cancelled call may leave a request in flight.
    pub async fn wait_for_visibility(
        &self,
        db: &str,
        repo: &str,
        version: u64,
        wait: VisibilityWait,
    ) -> Result<VisibilityState, ClientError> {
        crate::receipts::wait_for_visibility_via(
            |req| async move { self.request(&req).await },
            db,
            repo,
            version,
            wait,
        )
        .await
    }

    /// Status of a DDL operation, `None` if unknown.
    ///
    /// cancel-safe: NO — a cancelled call may leave a request in flight.
    pub async fn get_ddl_op_status(
        &self,
        db: &str,
        repo: &str,
        table: &str,
        op_id: &str,
    ) -> Result<Option<DdlOpStatus>, ClientError> {
        let req = DbRequest::GetDdlOpStatus {
            db: db.to_string(),
            repo: repo.to_string(),
            table: table.to_string(),
            op_id: op_id.to_string(),
        };
        match self.request(&req).await? {
            DbResponse::DdlOpStatus { status } => Ok(status),
            other => Err(ClientError::Protocol(format!(
                "expected DdlOpStatus, got {other:?}"
            ))),
        }
    }

    pub async fn close(self) {
        self.transport.close().await;
    }
}
