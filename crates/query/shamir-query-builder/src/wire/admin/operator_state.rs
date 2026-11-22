//! Operator-state request builders (admin-shell R5 PR3).
//!
//! Typed constructors for the two new top-level, read-only
//! [`DbRequest`] variants: the whole-server `status` observation (P1)
//! and the exact-key point read `get_record` (P2, NOT a `BatchOp` —
//! no cursor is created and no cursor path is engaged). Callers never
//! hand-assemble wire values.

#![warn(missing_docs)]

use shamir_query_types::wire::DbRequest;
use shamir_types::types::record_id::RecordId;

/// Build the read-only operator status observation request (P1).
///
/// Carries no parameters: the reply ([`DbResponse::Status`] with the
/// [`OperatorStatus`] snapshot) describes the whole server, including
/// the boot-time stamp outcome captured by the launcher while the boot
/// lock was held — never re-read at request time. Requires a
/// superuser-class session; a token session never reaches it.
pub fn status() -> DbRequest {
    DbRequest::Status
}

/// Build the typed exact-key point-read request (P2).
///
/// The server applies the `Database` `Read` then `Table` `Read` ACLs
/// (ancestor check before any lookup), the repository quarantine, a
/// typed-key size cap and the existing result-size and in-flight
/// response controls. `key` is a `RecordId` (fixed 16-byte msgpack
/// `bin` on the wire); a malformed key or a missing record is a typed
/// refusal, not a panic and not a scan. The reply is
/// [`DbResponse::Record`] or a typed `not_found`.
pub fn get_record(
    db: impl Into<String>,
    repo: impl Into<String>,
    table: impl Into<String>,
    key: RecordId,
) -> DbRequest {
    DbRequest::GetRecord {
        db: db.into(),
        repo: repo.into(),
        table: table.into(),
        key,
    }
}
