//! R1-c — `run_follower_loop`: the background follower engine
//! (REPLICATION §4/§5.2/§5.3/§5.6).
//!
//! Responsibilities, in order per iteration:
//!   1. [`hello`](super::source::ReplSource::hello) once at start to seed the
//!      max-seen `leader_epoch` (VR-style fencing, §5.2).
//!   2. Loop (until cancelled):
//!      - read the durable `replication_bookmark` for `(db, repo)` on the
//!        follower;
//!      - `pull(db, repo, bookmark+1, limit, wait_ms=poll_wait_ms)`;
//!      - verify/adopt upstream identity FIRST, before leader-epoch fencing;
//!      - **epoch-fence (§5.2):** if the reply's epoch is strictly lower
//!        than the max seen, terminate with [`ReplError::StaleLeaderEpoch`];
//!        otherwise record the new max;
//!      - **gap-stop (§5.3 / R1 simplification):** if the reply carries
//!        `gap_at: Some(g)` with `g > from_version`, the follower is
//!        permanently missing `[from_version, g)` — log at `warn!`/`error!`
//!        and terminate with [`ReplError::JournalGap`] WITHOUT applying any
//!        events in that same reply (the supervisor marks the subscription
//!        `resync_required`; full automated snapshot reseed is R2);
//!      - decode the events payload (`Vec<ReplicatedEvent>`);
//!      - apply the decoded batch through `apply_replicated_batch`: reserve
//!        a flushed local-version upper bound before any history write, then
//!        flush history once before the covering bookmark set+flush;
//!      - no cursor advancement or acknowledgement precedes bookmark flush;
//!      - if no events were returned, the leader already long-polled
//!        `wait_ms`, so a small backoff is optional but harmless.
//!
//! **§5.6 invariant:** this is a background task. It holds NO follower locks
//! and never blocks the follower's own commits. Transient transport errors
//! are logged + retried with backoff. Epoch/gap/rejection and reopen-required
//! errors terminate the loop; a dead destination gate is never retried.

mod config;
mod loop_body;
mod metrics;
#[cfg(test)]
mod test_hooks;
#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

pub use config::FollowerLoopConfig;
pub use loop_body::run_follower_loop;
pub(crate) use loop_body::STALL_VISIBILITY_THRESHOLD;
pub(crate) use metrics::note_source_created;
pub use metrics::{replication_metrics_snapshot, LeaderStreamMetrics};
#[cfg(test)]
pub(crate) use test_hooks::{
    run_follower_loop_with_identity_store, ApplyParkHook, IdentityStoreOverride,
};
