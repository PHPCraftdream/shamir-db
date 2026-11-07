//! Durability policy validation for database initialization.

use crate::{DbResult, SystemStoreConfig};
use shamir_engine::function::EnvPolicy;
use shamir_engine::repo::durability_policy::{
    effective_wal_segment_bytes, resolve, DurabilityPolicy,
};

use super::core::ShamirDb;

impl ShamirDb {
    /// Initialize with a server-default durability policy.
    ///
    /// Invalid retention budgets and unsupported replication serving modes fail
    /// before opening the metadata repository. All repositories receive the
    /// policy before WAL recovery; later opens validate it again against the
    /// current effective segment size. Retention enforcement consumes the
    /// retention budget and overflow action in PR2.
    pub async fn init_with_durability(
        config: SystemStoreConfig,
        policy: DurabilityPolicy,
    ) -> DbResult<Self> {
        Self::init_inner(config, EnvPolicy::default(), false, policy).await
    }

    pub(super) fn resolve_durability(policy: DurabilityPolicy) -> DbResult<DurabilityPolicy> {
        resolve(&policy, None, effective_wal_segment_bytes())
    }
}
