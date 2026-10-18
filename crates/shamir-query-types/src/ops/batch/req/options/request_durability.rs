//! Per-request durability tier for a batch.

use serde::{Deserialize, Serialize};

/// Per-request durability tier (query-language v4).
///
/// - [`RequestDurability::Buffered`] (default / absent) — ack after the
///   in-memory MemBuffer; durability on the background tick or graceful
///   drain.
/// - [`RequestDurability::Synced`] — before ack, flush the durable backing
///   of every repo this batch touched, so a committed write survives even
///   an immediate hard crash.
///
/// The old `"async_index"` value is REMOVED from durability: the
/// index-apply visibility it controlled moved to the orthogonal
/// [`IndexVisibility`](super::IndexVisibility) option.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RequestDurability {
    /// Ack after the in-memory buffer (default).
    #[default]
    Buffered,
    /// Flush durable backing before ack.
    Synced,
}

impl RequestDurability {
    /// The wire name of this tier.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestDurability::Buffered => "buffered",
            RequestDurability::Synced => "synced",
        }
    }
}
