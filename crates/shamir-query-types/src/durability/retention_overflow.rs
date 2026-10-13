//! Action taken when a repo's WAL retention cap is exceeded.
//!
//! Shared DTO: carried by the engine's `DurabilityPolicy` (consumed in PR2
//! retention enforcement) and deserialisable from the server config /
//! per-repo override layer (PR-later).

use serde::{Deserialize, Serialize};

/// What a repo does when its WAL retention budget is exceeded.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RetentionOverflow {
    /// Stop admitting writes until the WAL shrinks below the cap (safe —
    /// the replication bound never regresses). Default.
    #[default]
    RefuseWrites,
    /// Keep accepting writes and publish a gap marker for the evicted
    /// journal prefix, letting followers re-seed from a snapshot.
    DeclareGap,
}
