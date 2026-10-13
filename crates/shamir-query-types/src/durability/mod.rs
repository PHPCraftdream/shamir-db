//! Durability DTOs shared between the engine and the server config layer:
//! the WAL commit tier, the retention-overflow action, the replication
//! serve mode, and the sparse per-repo override applied on top of a
//! server-default durability policy.

mod durability_override;
mod replication_serve;
mod retention_overflow;
mod wal_commit_tier;

pub use durability_override::DurabilityOverride;
pub use replication_serve::ReplicationServe;
pub use retention_overflow::RetentionOverflow;
pub use wal_commit_tier::WalCommitTier;

#[cfg(test)]
mod tests;
