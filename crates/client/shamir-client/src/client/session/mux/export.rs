//! [`Client`] methods for the online logical-dump export job (R3 step 2b):
//! `start_export` / `export_status` / `cancel_export` and the typed receipt
//! decode. The job is session-bound on the server; every send goes through
//! the ordinary [`Client::execute`](crate::Client::execute) path on THIS
//! session.
//!
//! Production servers enable the runner only in normal `ReadWrite` mode
//! with a configured private `security.export_root` and experimental
//! storage migrations disabled. After authorization and form checks, all
//! three operations return `online_export_not_configured` without a root.
//! With a root, `StartExport` returns `online_export_unavailable` on a
//! read-only server, with experimental storage migrations enabled, or when
//! the transport cannot tie cancellation to the connection lifetime. If
//! the runner is disabled with a configured root, status and cancel return
//! `online_export_not_found` because there is no visible job.
//!
//! Every operation uses `db` as the request database envelope. The start
//! payload names the database to export and must match that envelope; its
//! repository and table options narrow that database scope. Status and cancel
//! carry no database in their operation payloads and resolve a job only on
//! the same native SCRAM session and database envelope as its start.

use serde::de::DeserializeOwned;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::migration::{
    cancel_export as cancel_export_op, export_status as export_status_op,
};
use shamir_query_types::admin::{ExportCancelReceipt, ExportStartReceipt, ExportStatusReceipt};
use shamir_query_types::batch::{BatchOp, BatchResponse};
use shamir_query_types::read::QueryRecord;
use shamir_types::types::value::QueryValue;

use crate::client::Client;
use crate::error::ClientError;

const START_EXPORT_ALIAS: &str = "start_export";
const EXPORT_STATUS_ALIAS: &str = "export_status";
const CANCEL_EXPORT_ALIAS: &str = "cancel_export";

impl Client {
    /// Start an online logical-dump export job (R3 step 2b).
    ///
    /// The call runs on THIS client's native SCRAM session, and the job it
    /// starts is bound to that same session: [`Client::export_status`] and
    /// [`Client::cancel_export`] resolve only on the SAME native SCRAM
    /// session (and the same envelope database); no later session can
    /// observe the job.
    ///
    /// No output path is accepted on the wire: the server writes only
    /// under its configured private export root and reports only an
    /// opaque artifact name. Perimeter authorization denies the request
    /// with [`ClientError::Db`] (`permission_denied`); if the runner is
    /// enabled, its scope authorization denies it with `export_scope_denied`
    /// (including failed read/manage checks). Neither denial starts a job.
    /// A server without the export root configured answers the typed
    /// `online_export_not_configured` rejection; a request whose form is
    /// not a standalone single-op batch answers the typed
    /// `online_export_invalid_form` rejection.
    ///
    /// # Lost responses
    ///
    /// No blind retry, ever. A definitive pre-registration rejection
    /// such as `online_export_not_configured` means no job was started.
    /// An `internal_error` can occur after job registration and does not
    /// prove that no job started. An undecodable response
    /// ([`ClientError::Protocol`]), a request timeout
    /// ([`ClientError::RequestTimeout`]) or a connection close
    /// ([`ClientError::ConnectionClosed`]) also leaves the outcome unknown.
    /// Resolve an uncertain start with [`Client::export_status`] on the
    /// same session, never by resending the start. The server's job
    /// registry is authoritative. A request timeout reclaims the request
    /// ID without closing the session. After a connection close, no later
    /// session can observe that job, and this client makes no claim about
    /// post-close cleanup.
    ///
    /// # Cancel-safety
    ///
    /// cancel-safe: NO — dropping or cancelling this future after the
    /// batch was sent leaves the start's outcome UNKNOWN exactly like a
    /// lost response; resolve with `export_status`, never resend.
    pub async fn start_export(
        &self,
        db: &str,
        start: shamir_query_builder::ddl::migration::StartExport,
    ) -> Result<ExportStartReceipt, ClientError> {
        let response = self
            .execute_export_op(db, START_EXPORT_ALIAS, start.build())
            .await?;
        export_receipt(&response, START_EXPORT_ALIAS)
    }

    /// Query the status of this session's logical-export job (read-only;
    /// safe to repeat).
    ///
    /// The server reports the CURRENT (or most recent) job of its single
    /// server-wide slot — only while the visible record still belongs to
    /// this session and database, and only until the next start replaces
    /// the record; afterwards status answers the server's typed no-job
    /// answer. No state is guessed client-side. This is the ONLY
    /// sanctioned resolution path for an uncertain
    /// [`Client::start_export`] — see its lost-response contract.
    pub async fn export_status(&self, db: &str) -> Result<ExportStatusReceipt, ClientError> {
        let response = self
            .execute_export_op(db, EXPORT_STATUS_ALIAS, export_status_op())
            .await?;
        export_receipt(&response, EXPORT_STATUS_ALIAS)
    }

    /// Cancel this session's logical-export job.
    ///
    /// A success receipt with `cancelled == true` means the cancel REQUEST
    /// was accepted (the signal was delivered onto a live job) — NOT that
    /// the cleanup has finished; use [`Client::export_status`] to observe
    /// the cleanup and the terminal phase. A receipt with `cancelled ==
    /// false` means the visible job already reached its noncancellable
    /// publication window or terminal state (including a successful
    /// post-publication cancel): the artifact stands, nothing was undone.
    /// A definitive [`ClientError::Db`] rejection is final;
    /// a lost response leaves the cancel's effect unknown (the job may or
    /// may not have observed it) — resolve with [`Client::export_status`],
    /// never by resending the cancel. The same loss-class limits as
    /// [`Client::start_export`] apply: a decode failure or a request
    /// timeout is resolvable on THIS session; a connection close cannot
    /// resolve the session-bound job at all (no later session can observe
    /// it).
    pub async fn cancel_export(&self, db: &str) -> Result<ExportCancelReceipt, ClientError> {
        let response = self
            .execute_export_op(db, CANCEL_EXPORT_ALIAS, cancel_export_op())
            .await?;
        export_receipt(&response, CANCEL_EXPORT_ALIAS)
    }

    /// Send ONE typed export admin operation as a single-op batch through
    /// the ordinary `execute` path (interner/epoch handling untouched).
    async fn execute_export_op(
        &self,
        db: &str,
        alias: &str,
        op: BatchOp,
    ) -> Result<BatchResponse, ClientError> {
        let mut batch = Batch::new();
        batch.op(alias, op);
        self.execute(
            db,
            batch
                .try_build()
                .map_err(crate::error::client_batch_error)?,
        )
        .await
    }
}

/// Decode a typed export receipt out of the response's single direct
/// record under the EXACT operation alias — the same wire convention as
/// the schema-migration receipt decode: the server answers one
/// [`shamir_query_types::read::QueryResult`] whose sole record is a
/// direct [`shamir_types::types::value::QueryValue::Map`] with named serde
/// fields; it is re-encoded with `rmp_serde::to_vec_named` and decoded into
/// the typed DTO. Exactly one result under exactly the sent alias and
/// exactly one direct MAP record are accepted — anything else (missing
/// alias, extra result aliases, several records, non-direct or non-map
/// record, undecodable payload) fails CLOSED with a STATIC message that
/// carries only the local operation alias; serde decode detail is dropped
/// because rmp-serde error text can embed untrusted wire values.
fn export_receipt<T: DeserializeOwned>(
    response: &BatchResponse,
    alias: &str,
) -> Result<T, ClientError> {
    let result = response.results.get(alias).ok_or_else(|| {
        ClientError::Protocol(format!("{alias} result missing from batch response"))
    })?;
    if response.results.len() != 1 {
        return Err(ClientError::Protocol(format!(
            "{alias} result must be the ONLY result in the batch response"
        )));
    }
    let [record] = result.records.as_slice() else {
        return Err(ClientError::Protocol(format!(
            "{alias} must carry exactly one record"
        )));
    };
    let QueryRecord::Direct(payload) = record else {
        return Err(ClientError::Protocol(format!(
            "{alias} payload is not a direct record"
        )));
    };
    let QueryValue::Map(payload) = payload else {
        return Err(ClientError::Protocol(format!(
            "{alias} payload is not a direct map record"
        )));
    };
    let bytes = rmp_serde::to_vec_named(payload)
        .map_err(|_| ClientError::Protocol(format!("{alias} payload encode failed")))?;
    rmp_serde::from_slice(&bytes)
        .map_err(|_| ClientError::Protocol(format!("{alias} payload decode failed")))
}
