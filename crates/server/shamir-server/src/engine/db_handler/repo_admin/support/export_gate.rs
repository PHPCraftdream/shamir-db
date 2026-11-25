//! Online-export wire dispatch helpers (R3 step 2b) for the db_handler.
//!
//! This file owns the *wire-level* preflight for the `StartExport` /
//! `ExportStatus` / `CancelExport` admin operations. The caller
//! (`execute_with_rid`) calls [`export_dispatch`] and drives the job
//! from the returned [`ExportDispatch`].
//!
//! Form contract (enforced atomically, before any effect): an export
//! operation is valid ONLY as a standalone, nontransactional, top-level
//! batch with exactly one entry. Any nested `Batch`/`ForEach` body, any
//! mixed batch (an export op beside any other op), a transactional
//! batch, or an envelope mismatch (the op's `database` field naming a
//! database other than the request's target) is rejected as a whole with
//! a single static error — no partial execution, no gate taken.
//!
//! Authorization ordering: the SCRAM + superuser check runs BEFORE any
//! form/receipt detail is distinguishable, and the not-configured refusal
//! (`online_export_not_configured`) stays the stable no-configured code.
//! Node-mode / experimental-migration capability gates are a separate
//! slice; none are consulted here.
//!
//! Response convention: the receipt is returned as a typed result under
//! the EXISTING batch convention — the result map is keyed by the query's
//! own alias (the pre-existing, documented batch result key), not by a
//! fixed alias. The server echoes no other peer text: every error message
//! in this file is a fixed string; the alias, the database name, and any
//! path are never included.
//!
//! Typed receipt encoding: [`receipt_value`] maps an `ExportStartReceipt`
//! / `ExportStatusReceipt` / `ExportCancelReceipt` DTO onto a
//! [`QueryValue`] via `rmp_serde::to_vec_named` →
//! `rmp_serde::from_slice::<QueryValue>` (the same codec the wire uses, so
//! it is lossless and byte-identical). Msgpack null behavior was checked,
//! not patched: every `Option` field on the three receipt DTOs is
//! `skip_serializing_if = "Option::is_none"`, so no nil-valued keys are
//! ever emitted, and the `Value` deserializer already maps a bare nil to
//! `QueryValue::Null` (`visit_none` / `visit_unit`), so no fix is needed.

use shamir_connect::server::session::Session;
use shamir_connect::server::SessionOrigin;
use shamir_db::query::batch::{BatchOp, BatchRequest};
use shamir_query_types::admin::StartExportOp;
use shamir_query_types::batch::BatchTreeVisitor;
use shamir_types::types::value::QueryValue;

use crate::db_handler::handler::DbResponse;

/// The recognized export operation of a valid dispatch form. `Start`
/// borrows the op payload from the batch; the `Status` / `Cancel`
/// wire payloads are presence-only discriminators with no fields, so
/// they are carried as unit variants.
///
/// Recognition covers top-level leaves only: the form validator has
/// already rejected every nested or mixed shape before this value is
/// produced, so no nested variant exists here.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ExportDispatch<'a> {
    /// Start a new export job.
    Start(&'a StartExportOp),
    /// Query the calling session's job status (read-only).
    Status,
    /// Request cancellation of the calling session's job.
    Cancel,
}

/// Atomic wire preflight for the online-export operations.
///
/// * `Ok(None)` — the batch contains no export operation at any depth;
///   the caller proceeds with the normal batch path.
/// * `Ok(Some(dispatch))` — the batch is a valid standalone export
///   request from a SCRAM superuser session; `dispatch` borrows the op
///   payload. The admission chunk resolves the job from here.
/// * `Err(response)` — atomic refusal with a stable code and a fixed
///   message; no effect was taken and none may be.
///
/// Ordering inside this function is the contract: SCRAM + superuser first
/// (so a non-privileged caller learns nothing about form or
/// configuration), then form, then the envelope equality check.
pub(crate) fn export_dispatch<'a>(
    session: &Session,
    batch: &'a BatchRequest,
    db_name: &str,
) -> Result<Option<ExportDispatch<'a>>, Box<DbResponse>> {
    let has_export_op = BatchTreeVisitor::new(&batch.queries).any(|visit| {
        matches!(
            visit.entry.op,
            BatchOp::StartExport(_) | BatchOp::ExportStatus(_) | BatchOp::CancelExport(_)
        )
    });
    if !has_export_op {
        return Ok(None);
    }
    if let Some(refusal) = scram_superuser_refusal(session) {
        return Err(Box::new(refusal));
    }
    let dispatch = export_dispatch_form(batch, db_name)?;
    Ok(Some(dispatch))
}

/// SCRAM + session-superuser gate for the export surface.
///
/// `Some(refusal)` when the session is not SCRAM-authenticated or does
/// not carry the superuser flag; the message is fixed and names neither
/// the user, the origin, nor any other session detail. `ExportStatus` is
/// gated identically to `StartExport` / `CancelExport` — the visibility
/// of job state is privileged even though the operation is read-only.
pub(crate) fn scram_superuser_refusal(session: &Session) -> Option<DbResponse> {
    if !matches!(session.origin, SessionOrigin::Scram) {
        return Some(DbResponse::Error {
            code: "permission_denied".into(),
            message: "online export operations require a SCRAM session".into(),
        });
    }
    if !session.permissions.is_superuser {
        return Some(DbResponse::Error {
            code: "permission_denied".into(),
            message: "online export operations require superuser".into(),
        });
    }
    None
}

/// Form validation for a batch already known to contain an export op and
/// to come from a SCRAM superuser session.
///
/// Accepts exactly one shape: a nontransactional top-level batch with a
/// single entry whose op is one of the three export operations and whose
/// `database` field equals the request's target database (the wire
/// payload must equal the envelope, no `when` and no `after`). Everything
/// else — nested bodies, mixed batches, transactional batches, envelope
/// mismatch — is one
/// atomic `online_export_invalid_form` refusal with a fixed message.
fn export_dispatch_form<'a>(
    batch: &'a BatchRequest,
    db_name: &str,
) -> Result<ExportDispatch<'a>, Box<DbResponse>> {
    if batch.transactional {
        return Err(form_refusal());
    }
    let mut dispatch: Option<ExportDispatch> = None;
    for visit in BatchTreeVisitor::new(&batch.queries) {
        if visit.depth != 0 {
            // A nested `Batch`/`ForEach` body under an export op (or an
            // export op inside one): never a valid export form.
            return Err(form_refusal());
        }
        if dispatch.is_some() {
            // A second entry: mixed batch.
            return Err(form_refusal());
        }
        if visit.entry.when.is_some() || !visit.entry.after.is_empty() {
            // The direct dispatch bypasses the planner: a condition or a
            // dependency would be ignored, not honoured.
            return Err(form_refusal());
        }
        dispatch = Some(match &visit.entry.op {
            BatchOp::StartExport(op) => ExportDispatch::Start(op),
            BatchOp::ExportStatus(_) => ExportDispatch::Status,
            BatchOp::CancelExport(_) => ExportDispatch::Cancel,
            // Non-export entry beside an export op: mixed batch. (The
            // caller pre-checked that at least one export op exists, so
            // reaching a non-export entry here means there were both.)
            _ => return Err(form_refusal()),
        });
    }
    let dispatch = dispatch.ok_or_else(form_refusal)?;
    if let ExportDispatch::Start(start) = &dispatch {
        if start.database != db_name {
            // Envelope equality: the payload's database must be the
            // database the request was addressed to. The mismatch detail
            // (either name) is never echoed.
            return Err(form_refusal());
        }
    }
    Ok(dispatch)
}

/// The single atomic form refusal. Fixed code and message; no alias, no
/// database name, no path, no peer text.
fn form_refusal() -> Box<DbResponse> {
    Box::new(DbResponse::Error {
        code: "online_export_invalid_form".into(),
        message: "online export operations require a standalone, \
                  nontransactional, top-level single-operation batch"
            .into(),
    })
}

/// Typed "export root not configured" refusal — the stable
/// `online_export_not_configured` code, shared by start / status / cancel.
pub(crate) fn not_configured_refusal() -> DbResponse {
    DbResponse::Error {
        code: "online_export_not_configured".into(),
        message: "online export is not configured on this server".into(),
    }
}

/// Encode a typed export receipt DTO (`ExportStartReceipt`,
/// `ExportStatusReceipt`, `ExportCancelReceipt`) into a [`QueryValue`]
/// via the wire msgpack codec (`to_vec_named` → `from_slice`), the same
/// round-trip the admin dispatch surface uses for DTO-valued admin
/// results.
///
/// Errors are internal encoding failures only (the DTOs are plain maps of
/// scalars); they surface as the fixed `internal_error` code with a
/// static message. See the module docs for the checked null behavior.
pub(crate) fn receipt_value(
    receipt: &impl serde::Serialize,
) -> Result<QueryValue, Box<DbResponse>> {
    let bytes = rmp_serde::to_vec_named(receipt).map_err(|_| internal_refusal())?;
    rmp_serde::from_slice::<QueryValue>(&bytes).map_err(|_| internal_refusal())
}

/// The fixed internal-error response used only for impossible codec
/// failures; it carries no detail.
fn internal_refusal() -> Box<DbResponse> {
    Box::new(DbResponse::Error {
        code: "internal_error".into(),
        message: "failed to encode the online export response".into(),
    })
}
