//! [`Client`] methods for the online logical-dump export job (R3 step 2b):
//! `start_export` / `export_status` / `cancel_export` and the typed receipt
//! decode. The job is session-bound on the server; every send goes through
//! the ordinary [`Client::execute`](crate::Client::execute) path on THIS
//! session.
//!
//! STATUS: future protocol shape. This server build's handler only exposes
//! the fail-closed `permission_denied` / `online_export_not_configured`
//! refusals (the live job is gated by storage-owned buffer maintenance that
//! lacks its admission API), so the methods below are typed to the agreed
//! wire contract but MUST NOT be documented as working against a live job
//! yet; a server without the job configured answers the typed
//! `online_export_not_configured` rejection. Node-mode / follower
//! capability gates are a separate server slice and are NOT consulted by
//! this surface, so no such code is documented here. The paragraphs
//! below describe the INTENDED protocol behavior for a server build that admits the job;
//! no deployment-specific claim is made about any build that answers the
//! fail-closed refusals.
//!
//! The `db` parameter of every operation is the node-local envelope
//! placeholder, exactly like the application-schema migration control
//! calls: `StartExport` / `ExportStatus` / `CancelExport` are NOT
//! database-scoped on the server (the export scope travels flat inside the
//! start op), so the envelope database names a database the session can
//! address and nothing else.

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
    /// opaque artifact name. Requires the superuser plus the
    /// included-scope read/manage checks on the server; denial arrives as
    /// [`ClientError::Db`] (`permission_denied`) and nothing was started.
    /// A server without the export root configured answers the typed
    /// `online_export_not_configured` rejection; a request whose form is
    /// not a standalone single-op batch answers the typed
    /// `online_export_invalid_form` rejection.
    ///
    /// # Lost responses
    ///
    /// No blind retry, ever. A DEFINITIVE server rejection
    /// ([`ClientError::Db`] — including the typed
    /// `online_export_not_configured` refusal when the server has no
    /// export root configured) means no job was started and the call may
    /// be repeated deliberately by the caller. Anything observed AFTER
    /// the batch was sent leaves the outcome UNKNOWN — an undecodable
    /// response ([`ClientError::Protocol`]), a request timeout
    /// ([`ClientError::RequestTimeout`], which reclaims the request id
    /// without closing the session) or a connection close
    /// ([`ClientError::ConnectionClosed`]) — and MUST be resolved with
    /// [`Client::export_status`], never by resending the start (a second
    /// start is refused with the typed in-progress code while the first
    /// job lives). Resolution limits by loss class: a decode failure or a
    /// request timeout leaves THIS session alive, so `export_status` on
    /// the SAME session resolves the outcome (the server's job registry is
    /// the authority on whether the start took effect). A connection
    /// close cannot resolve a session-bound job at all: no later session
    /// can observe that job, and this client makes no claim about
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
    let result = response
        .results
        .get(alias)
        .ok_or_else(|| {
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
