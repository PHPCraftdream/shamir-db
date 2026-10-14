//! Optional observation port for successful repository durability changes.

use crate::engine::repo::DurabilityPolicy;

/// Synchronous per-operation notification after an ALTER REPO durability
/// policy has been persisted and installed, before result filtering.
pub trait RepoAdminObserver: Send + Sync {
    /// Called once for each successfully persisted and installed durability ALTER.
    fn repository_durability_changed(
        &self,
        db_name: &str,
        repo: &str,
        effective: &DurabilityPolicy,
    );
}
