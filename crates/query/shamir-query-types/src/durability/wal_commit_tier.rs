//! WAL commit tier chosen by a repo's durability policy.
//!
//! Shared DTO: the engine's `DurabilityPolicy` (PR1 Chunk B) carries this
//! value per repo; the server config / per-repo override layer (PR-later)
//! deserialises the same shape from the wire, so the enum lives here
//! alongside the other wire-facing enums.

use serde::{Deserialize, Serialize};

/// Which durability tier a repo's commits ack at.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WalCommitTier {
    /// Ack after the group-commit `write()` (level 2): survives a process
    /// crash; the nominal 250 ms fsync timer is not a hard power-loss bound.
    /// This is the default.
    #[default]
    Buffered,
    /// Ack after the group-commit `fsync()` (level 3): survives power loss.
    /// Each group-commit window that carries a `Synced` waiter issues one
    /// fsync (coalesced across concurrent committers).
    Synced,
}
