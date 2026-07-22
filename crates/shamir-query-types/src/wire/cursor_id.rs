//! [`CursorId`] — opaque handle for a server-side result cursor (FG-5).

use serde::{Deserialize, Serialize};

/// Opaque server-assigned handle for a paginated result cursor.
///
/// Minted by the server in response to [`crate::wire::db_message::DbRequest::CreateCursor`]
/// (carried back in [`crate::wire::db_message::DbResponse::CursorPage`]) and echoed by the
/// client on every subsequent [`crate::wire::db_message::DbRequest::FetchNext`] /
/// [`crate::wire::db_message::DbRequest::CancelCursor`] call. The wire representation is a
/// bare `u64` (a newtype, not a struct-wrapped object) — mirrors how `tx_handle` and
/// subscription `sub` ids are already carried on the wire elsewhere in this crate — so it
/// round-trips as a plain integer, not `{ "0": N }`.
///
/// The `u64` value has no meaning to clients beyond equality/echo — it does not encode
/// session, snapshot, or session-affinity information on the wire (that lives server-side,
/// FG-5b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CursorId(pub u64);

impl std::fmt::Display for CursorId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for CursorId {
    fn from(id: u64) -> Self {
        CursorId(id)
    }
}

impl From<CursorId> for u64 {
    fn from(id: CursorId) -> Self {
        id.0
    }
}
