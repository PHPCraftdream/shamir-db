//! Wire result of attempting cooperative request cancellation.

use serde::{Deserialize, Serialize};

/// Result of a cancellation attempt, distinct from the target batch's reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelOutcome {
    /// Cancellation won; the target stops cooperatively.
    Cancelled,
    /// The request is unknown or no longer running.
    AlreadyFinished,
    /// Commit won the race; observe the target's normal commit outcome.
    AlreadyCommitting,
    /// This work cannot be cancelled (for example, follower replication apply).
    NotCancellable,
}
