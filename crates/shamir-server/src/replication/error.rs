//! R1-c — error type for the follower replication pull-loop.
//!
//! Splits into two classes:
//!   * **Terminal** (epoch regression, journal gap, or apply rejection)
//!     — the loop MUST stop. A leader whose epoch regressed is a fencing
//!     violation (REPLICATION §5.2); continuing would risk applying events
//!     from a deposed leader. A journal gap means the follower is missing
//!     data it can never recover by continuing to pull — silently skipping
//!     past it would leave the follower permanently and invisibly
//!     inconsistent, so this is now a hard stop too (see [`ReplError::JournalGap`]).
//!   * **Transient** (everything else) — transport hiccups, decode failures,
//!     apply errors. The loop logs and backs off, then retries; §5.6 makes
//!     replication a non-fatal background task.

use shamir_db::engine::tx::apply_replicated::ApplyRejectReason;
use thiserror::Error;

/// Error returned by [`ReplSource`](super::source::ReplSource) implementations
/// and the [`run_follower_loop`](super::follower_loop::run_follower_loop)
/// engine.
#[derive(Debug, Error)]
pub enum ReplError {
    #[error("{0}")]
    Seed(#[from] shamir_db::SeedRefusal),
    #[error("replication start failed: {0}")]
    Start(String),

    #[error("destination {db}/{repo} requires reopen: {detail}")]
    ReopenRequired {
        db: String,
        repo: String,
        detail: String,
    },

    #[error("destination {db}/{repo} already has replication owner {owner}")]
    DuplicateOwner {
        db: String,
        repo: String,
        owner: String,
    },

    /// VR-style fencing trip (§5.2): the source returned a `leader_epoch`
    /// strictly lower than a previously observed one. The follower treats
    /// this as a stale ("resurrected") leader and terminates the loop —
    /// re-establishing replication requires a fresh connection with a
    /// monotonic epoch.
    #[error(
        "stale leader epoch: observed {observed} but previously saw {max_seen} \
         (VR-style fencing, REPLICATION §5.2)"
    )]
    StaleLeaderEpoch {
        /// The regressed epoch carried by the offending response.
        observed: u64,
        /// The highest epoch the loop had previously recorded.
        max_seen: u64,
    },

    /// The leader reported a journal gap (`gap_at`) — events in
    /// `[from_version, gap_at)` are no longer retained. **Terminal**: the
    /// follower is permanently missing this range and continuing to pull
    /// would silently resume past the loss. The loop MUST stop rather than
    /// skip-and-continue; the caller (the supervisor) marks the
    /// subscription `resync_required` so the gap is visible via the
    /// existing admin surface. Recovery is a manual operator step (verify/
    /// fix the follower's data, then `Resume`); full automated snapshot
    /// reseed remains R2.
    #[error(
        "journal gap at leader version {gap_at} (requested from {from_version}); \
         stopping the follower loop rather than silently skipping the missing \
         range (full snapshot reseed is R2)"
    )]
    JournalGap {
        /// Lowest retained version reported by the leader.
        gap_at: u64,
        /// The `from_version` the follower had requested.
        from_version: u64,
    },

    /// Permanent follower incompatibility. Stop without advancing the
    /// bookmark; the supervisor marks the subscription resync_required.
    #[error("replicated event at leader version {leader_version} rejected: {reason:?}")]
    ApplyRejected {
        leader_version: u64,
        reason: ApplyRejectReason,
    },

    /// S5c-X PR7 — the universal leader-identity fence tripped: the leader's
    /// `(lineage, history_epoch)` differs from the identity recorded at this
    /// subscription's first contact (or a seed receipt). **Terminal**: the
    /// loop stops, NOTHING from the offending response is applied, and the
    /// supervisor persists the typed reason plus the recorded and observed
    /// identities on the subscription row. Recovery is an operator step
    /// (reseed); the loop never retries blindly against a changed identity.
    #[error(
        "leader identity mismatch ({code}): recorded ({recorded}, history epoch {recorded_epoch}), observed ({observed}, history epoch {observed_epoch}); stopping the follower loop - operator action (verify the leader history, then reseed) required"
    )]
    IdentityMismatch {
        /// Which comparison tripped (`seed_wrong_identity` for a lineage
        /// change, `seed_wrong_history_epoch` for an epoch change).
        code: shamir_db::SeedRefusal,
        /// Recorded lineage id, hex.
        recorded: String,
        /// Recorded history epoch.
        recorded_epoch: u64,
        /// Observed lineage id, hex.
        observed: String,
        /// Observed history epoch.
        observed_epoch: u64,
    },

    /// Transport-level failure (connection drop, decode error, wire error
    /// envelope). Transient — the loop backs off and retries.
    #[error("replication transport error: {0}")]
    Transport(String),

    /// The `events` payload failed to decode as
    /// `Vec<ChangelogEvent>` (msgpack). Transient — treated as a transport
    /// corruption and retried.
    #[error("failed to decode changelog events payload: {0}")]
    Decode(String),

    /// Applying a replicated event to the follower store failed. Transient
    /// by default — a re-delivery after backoff may succeed, and
    /// `apply_replicated` is idempotent so retries are safe.
    #[error("applying replicated event (leader commit_version {leader_version}) failed: {source}")]
    Apply {
        /// The leader `commit_version` of the event that failed to apply.
        leader_version: u64,
        /// Underlying storage error.
        #[source]
        source: shamir_db::storage::error::DbError,
    },

    /// The follower repo does not exist on the local `ShamirDb`. Not
    /// transient in itself, but the loop treats it as such (the repo may
    /// be created later by an admin DDL op) and backs off.
    #[error("follower repo '{db}/{repo}' not found")]
    UnknownFollowerRepo {
        /// Database name.
        db: String,
        /// Repository name.
        repo: String,
    },

    /// Reading the durable replication bookmark failed. Transient — retried
    /// after backoff.
    #[error("failed to read replication bookmark: {0}")]
    Bookmark(String),
}
