//! The per-repo durability policy value type (PR1 Chunk B + amendment B3).
//!
//! Carried as a shared `Arc<ArcSwap<…>>` field on `RepoInstance`, installed
//! before recovery and admission. Configured and backend-effective tiers
//! travel together in the same policy snapshot.

use shamir_query_types::{ReplicationServe, RetentionOverflow, WalCommitTier};

mod recoverability_coverage;
mod recovered_open;
mod replication_page;
pub(crate) use recovered_open::{
    certify_recovered_open, checkpoint_allocation_floor, install_empty_coverage,
};
pub(crate) use replication_page::read_replication_page;
mod segment_bytes;
pub(crate) use recoverability_coverage::RecoverabilityCoverage;
pub use segment_bytes::effective_wal_segment_bytes;

/// Default WAL retention budget: 1 GiB.
pub const DEFAULT_WAL_RETENTION_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// Per-repo durability policy.
///
/// `wal_commit` retains the configured tier for telemetry; the commit seam
/// consumes `effective_wal_commit` from the same snapshot.
/// The retention fields are **consumed in PR2** (WAL retention enforcement)
/// and are carried here so the wiring lands once — their accessors keep
/// them live (no dead code) until PR2 starts reading them.
/// `replication_serve` is consumed by the replication bound (only
/// `Durable` is accepted in PR1 — see `resolve`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DurabilityPolicy {
    wal_commit: WalCommitTier,
    effective_wal_commit: WalCommitTier,
    wal_retention_max_bytes: u64,
    on_wal_retention_exceeded: RetentionOverflow,
    replication_serve: ReplicationServe,
}

impl DurabilityPolicy {
    /// Explicit constructor. `Default` is buffered / 1 GiB / refuse-writes /
    /// durable serving.
    #[must_use]
    pub const fn new(
        wal_commit: WalCommitTier,
        wal_retention_max_bytes: u64,
        on_wal_retention_exceeded: RetentionOverflow,
        replication_serve: ReplicationServe,
    ) -> Self {
        Self {
            wal_commit,
            effective_wal_commit: wal_commit,
            wal_retention_max_bytes,
            on_wal_retention_exceeded,
            replication_serve,
        }
    }

    /// Configured tier, retained even when the backend has a Mem WAL.
    #[must_use]
    pub const fn wal_commit(&self) -> WalCommitTier {
        self.wal_commit
    }

    /// Backend-effective tier read by the commit seam.
    #[must_use]
    pub const fn effective_wal_commit(&self) -> WalCommitTier {
        self.effective_wal_commit
    }

    pub(crate) fn for_wal_backend(mut self, file_wal: bool) -> Self {
        self.effective_wal_commit = if file_wal {
            self.wal_commit
        } else {
            WalCommitTier::Buffered
        };
        self
    }

    /// WAL retention budget in bytes. **Consumed in PR2** (retention
    /// enforcement); exposed now so the wiring is stable.
    #[must_use]
    pub const fn wal_retention_max_bytes(&self) -> u64 {
        self.wal_retention_max_bytes
    }

    /// Action when the retention budget is exceeded. **Consumed in PR2**;
    /// exposed now so the wiring is stable.
    #[must_use]
    pub const fn on_wal_retention_exceeded(&self) -> RetentionOverflow {
        self.on_wal_retention_exceeded
    }

    /// What a replication pull may serve past the WAL's durability boundary
    /// (config key `durability.replication_serve`). PR1 accepts ONLY
    /// [`ReplicationServe::Durable`] — `resolve` rejects the rest.
    #[must_use]
    pub const fn replication_serve(&self) -> ReplicationServe {
        self.replication_serve
    }
}

impl Default for DurabilityPolicy {
    /// `Buffered` commit tier, 1 GiB retention budget, refuse writes on
    /// overflow, durable-only replication serving.
    fn default() -> Self {
        Self::new(
            WalCommitTier::Buffered,
            DEFAULT_WAL_RETENTION_MAX_BYTES,
            RetentionOverflow::RefuseWrites,
            ReplicationServe::Durable,
        )
    }
}
