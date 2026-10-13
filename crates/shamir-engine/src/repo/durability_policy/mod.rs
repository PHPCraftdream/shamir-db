//! Per-repo durability policy (PR1 Chunk B + amendments B2/B3 — engine
//! layer).
//!
//! [`DurabilityPolicy`] is the value type every repo carries: which WAL
//! commit tier commits ack at, what a replication pull may serve
//! (`durability.replication_serve`, only `durable` in PR1), and (consumed
//! in PR2) how much WAL the repo may retain and what to do when the
//! retention cap is exceeded.
//!
//! The typed enums (`WalCommitTier`, `RetentionOverflow`,
//! `ReplicationServe`) are wire-facing and live in
//! `shamir_query_types::durability`; they are re-exported here so
//! engine-internal paths (and tests) keep a stable import site.
//!
//! [`resolve`] folds a server-default policy plus an optional
//! `DurabilityOverride` into the effective policy, rejecting
//! `completed_reseed` (not implemented in PR1) and enforcing the
//! retention-vs-segment-size minimum.
//!
//! The replication-bound helper [`replication_bound`] implements the I3
//! invariant: a replication pull may never serve a version outside `R`, the
//! WAL's contiguous recoverability prefix, because versions above `R` do
//! not survive a power loss while the follower's bookmark does. Serve bound
//! = `min(confirmed, W, R)`.

mod durability_metrics;
mod durability_policy;
mod replication_bound;
mod resolve;

pub use durability_metrics::{DurabilityMetrics, DurabilityMetricsSnapshot};
pub use durability_policy::effective_wal_segment_bytes;
#[cfg(test)]
pub(crate) use durability_policy::RecoverabilityCoverage;
pub(crate) use durability_policy::{
    certify_recovered_open, checkpoint_allocation_floor, install_empty_coverage,
    read_replication_page,
};
pub use durability_policy::{DurabilityPolicy, DEFAULT_WAL_RETENTION_MAX_BYTES};
pub(crate) use replication_bound::replication_bound_observed;
pub use replication_bound::{replication_bound, REPLICATION_BOUND_WAIT};
pub use resolve::resolve;

// Amendments B2/B3: the typed enums moved to shamir-query-types
// (wire-facing); re-exported so existing engine paths keep their import
// site.
pub use shamir_query_types::{ReplicationServe, RetentionOverflow, WalCommitTier};

#[cfg(test)]
mod tests;
