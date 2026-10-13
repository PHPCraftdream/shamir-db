//! [`ChangesSinceEvent`] — client-side mirror of one changelog journal
//! event returned by the `changes_since` admin op.

use serde::{Deserialize, Serialize};
use shamir_types::access::Actor;

use super::record_change::ChangesSinceRecordChange;

/// One committed transaction, projected for the changelog journal, as it
/// appears inside a `changes_since` response.
///
/// Faithful wire mirror of `shamir_tx::changefeed::ChangelogEvent` (the
/// crate cannot depend on `shamir-tx`): field names, order and encodings
/// are identical, so the server's msgpack round-trip (`to_qv`) decodes
/// losslessly into this DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesSinceEvent {
    /// Repository name this commit belongs to.
    pub repo: String,
    /// MVCC commit version assigned to this tx (monotonic per repo).
    #[serde(deserialize_with = "super::u64_wire::deserialize")]
    pub commit_version: u64,
    /// The committing transaction's id.
    #[serde(deserialize_with = "super::u64_wire::deserialize")]
    pub tx_id: u64,
    /// The actor that initiated the transaction.
    pub actor: Actor,
    /// Wall-clock nanoseconds at projection time (best-effort, for
    /// observability — NOT used for ordering; `commit_version` is).
    #[serde(deserialize_with = "super::u64_wire::deserialize")]
    pub timestamp_ns: u64,
    /// Per-record changes carried by this commit.
    pub changes: Vec<ChangesSinceRecordChange>,
}
