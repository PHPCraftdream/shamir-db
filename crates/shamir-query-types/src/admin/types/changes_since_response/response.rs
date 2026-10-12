//! [`ChangesSinceResponse`] — typed decode of the `changes_since` admin
//! response payload.

use serde::{Deserialize, Serialize};
use shamir_types::types::value::QueryValue;

use super::event::ChangesSinceEvent;

/// Typed response of the `changes_since` admin op (temporal T4).
///
/// Mirrors the payload `ShamirDb::handle_changes_since` emits. `gap_at` is
/// the CF-1 re-sync marker (`None` = no gap). `visible_through` is the
/// settled prefix W sampled BEFORE the journal read (VIS-4c) — omitted by
/// older servers when the repo gate is not initialized, hence
/// `#[serde(default)]` so payloads from older servers still decode.
///
/// u64 PRECISION: this DTO decodes u64 positions exactly, in every form the
/// `QueryValue` round-trip produces (msgpack uint/int, or the decimal
/// string of `QueryValue::Big` for values above `i64::MAX`). NOTE: the
/// server currently SAMPLES and CASTS these positions through `i64`
/// (`admin_retention.rs`), so values above `i64::MAX` are not producible
/// by the server today — the lossless decode is a client-side guarantee,
/// not an end-to-end wire property yet.
///
/// SEMANTICS: this is a settlement sample, NOT a coverage certificate.
/// An empty `events` list is not proof that a pending write aborted or
/// vanished — journal confirmation may lag `visible_through`. Events stay
/// subject to the cursor/limit/gaps/retention. Do NOT advance your event
/// cursor to `visible_through` from this field alone; `WaitForVisibility`
/// remains the primary reconciliation operation, and `gap_at` / retention
/// truncation still require resynchronisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesSinceResponse {
    /// Echoed cursor: events carry `commit_version` strictly greater.
    #[serde(deserialize_with = "super::u64_wire::deserialize")]
    pub changes_since: u64,
    /// Journal events after the cursor (bounded by the op's `limit`).
    pub events: Vec<ChangesSinceEvent>,
    /// First missing journal version (re-sync marker); `None` = no gap.
    #[serde(default, deserialize_with = "super::u64_wire::deserialize_option")]
    pub gap_at: Option<u64>,
    /// Sampled settled prefix W at read time; absent on older servers.
    #[serde(
        default,
        deserialize_with = "super::u64_wire::deserialize_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub visible_through: Option<u64>,
}

impl ChangesSinceResponse {
    /// Decode the admin payload as the client receives it: the
    /// `QueryResult` record is the payload `QueryValue` the server built
    /// via its msgpack round-trip. Reverse of that round-trip.
    pub fn from_query_value(value: &QueryValue) -> Result<Self, String> {
        let bytes = rmp_serde::to_vec_named(value).map_err(|e| e.to_string())?;
        rmp_serde::from_slice(&bytes).map_err(|e| e.to_string())
    }
}
