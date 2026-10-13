//! What a replication pull may serve past the WAL's durability boundary.
//!
//! Shared DTO: carried by the engine's `DurabilityPolicy` (amendment B3,
//! point 3) and deserialisable from the server config / per-repo override
//! layer (PR-later). Config/policy key: `durability.replication_serve`.

use serde::{Deserialize, Serialize};

/// Replication serve mode for a repo.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationServe {
    /// Serve only versions inside the contiguous recoverability prefix:
    /// bound = `min(confirmed, W, R)`. A pull never serves a version a power
    /// loss could take back while the follower's bookmark survives. This is
    /// the (only, in PR1) mode.
    #[default]
    Durable,
    /// Serve completed-but-not-yet-covered versions, letting followers
    /// re-seed from a snapshot if the WAL tail is lost. NOT IMPLEMENTED in
    /// PR1: rejected by the engine's `resolve` with
    /// "completed_reseed is not available yet (planned: epoch bump +
    /// universal fencing)".
    CompletedReseed,
}
