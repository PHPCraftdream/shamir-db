//! ONE policy resolver (amendment B2, point 3; extended by B3 point 3):
//! fold a server-default [`DurabilityPolicy`] and an optional sparse
//! `DurabilityOverride` into the effective per-repo policy.
//!
//! PR1 always passes `None` (every repo runs the server default); the
//! override shape (`shamir_query_types::DurabilityOverride`) is already
//! serde-friendly so the PR-later per-repo admin op can reuse it verbatim.

use shamir_query_types::{DurabilityOverride, ReplicationServe};
use shamir_storage::error::DbError;

use super::durability_policy::DurabilityPolicy;

/// Minimum effective WAL retention budget: twice the effective segment
/// size. Below it the retention evictor could evict the very segment a
/// still-open writer is appending to (PR2 enforcement), so a resolved
/// policy under the floor is rejected here — at configuration time, not
/// at runtime.
pub const MIN_RETENTION_SEGMENT_FACTOR: u64 = 2;

/// Resolve the effective durability policy: start from `server_default`;
/// each `Some` field of `over` wins; then enforce the retention floor
/// `>= 2 × effective_segment_bytes`. `over = None` (the PR1 shape) returns
/// the server default unchanged (subject to the same floor check).
///
/// Amendment B3 (point 3): `replication_serve = completed_reseed` is
/// REJECTED here — boot and every later path go through `resolve`, so no
/// fast-serve mode can sneak in — with the exact message
/// "completed_reseed is not available yet (planned: epoch bump + universal fencing)".
///
/// The error names the configuration key, the configured value and the
/// computed minimum so an operator can fix the config without source
/// diving. Repository policy telemetry is sampled from its shared policy cell.
pub fn resolve(
    server_default: &DurabilityPolicy,
    over: Option<&DurabilityOverride>,
    effective_segment_bytes: u64,
) -> Result<DurabilityPolicy, DbError> {
    let mut policy = *server_default;
    if let Some(over) = over {
        if let Some(wal_commit) = over.wal_commit {
            policy = DurabilityPolicy::new(
                wal_commit,
                policy.wal_retention_max_bytes(),
                policy.on_wal_retention_exceeded(),
                policy.replication_serve(),
            );
        }
        if let Some(bytes) = over.wal_retention_max_bytes {
            policy = DurabilityPolicy::new(
                policy.wal_commit(),
                bytes.get(),
                policy.on_wal_retention_exceeded(),
                policy.replication_serve(),
            );
        }
        if let Some(action) = over.on_wal_retention_exceeded {
            policy = DurabilityPolicy::new(
                policy.wal_commit(),
                policy.wal_retention_max_bytes(),
                action,
                policy.replication_serve(),
            );
        }
        if let Some(serve) = over.replication_serve {
            policy = DurabilityPolicy::new(
                policy.wal_commit(),
                policy.wal_retention_max_bytes(),
                policy.on_wal_retention_exceeded(),
                serve,
            );
        }
    }
    if policy.replication_serve() == ReplicationServe::CompletedReseed {
        return Err(DbError::Internal(
            "completed_reseed is not available yet (planned: epoch bump + universal fencing)"
                .into(),
        ));
    }
    let minimum = effective_segment_bytes
        .saturating_mul(MIN_RETENTION_SEGMENT_FACTOR)
        .max(1);
    if policy.wal_retention_max_bytes() < minimum {
        return Err(DbError::Internal(format!(
            "wal_retention_max_bytes ({}) is below the minimum {} \
             (= {} × effective WAL segment size {} bytes)",
            policy.wal_retention_max_bytes(),
            minimum,
            MIN_RETENTION_SEGMENT_FACTOR,
            effective_segment_bytes,
        )));
    }
    Ok(policy)
}
