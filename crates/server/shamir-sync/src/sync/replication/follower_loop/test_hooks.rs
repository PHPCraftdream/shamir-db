use std::sync::Arc;

use shamir_db::ShamirDb;
use tokio_util::sync::CancellationToken;

use crate::replication::error::ReplError;
use crate::replication::source::ReplSource;

use super::config::FollowerLoopConfig;
use super::loop_body::run_follower_loop_internal;

/// Overrides only identity persistence for full-loop durability regressions.
#[cfg(test)]
pub(crate) struct IdentityStoreOverride {
    pub(crate) info: Arc<dyn shamir_db::storage::types::Store>,
    pub(crate) startup_reflush: bool,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ApplyParkHook {
    pub(crate) repo: Option<String>,
    pub(crate) after_apply: bool,
    pub(crate) arrived: Arc<tokio::sync::Notify>,
    pub(crate) release: Arc<tokio::sync::Notify>,
    pub(crate) exited: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
impl std::fmt::Debug for IdentityStoreOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityStoreOverride")
            .field("startup_reflush", &self.startup_reflush)
            .finish_non_exhaustive()
    }
}

/// Run the complete production loop body with a test-only identity store.
#[cfg(test)]
pub(crate) async fn run_follower_loop_with_identity_store(
    db: Arc<ShamirDb>,
    source: Arc<dyn ReplSource>,
    cfg: FollowerLoopConfig,
    cancel: CancellationToken,
    identity_store: IdentityStoreOverride,
) -> Result<(), ReplError> {
    run_follower_loop_internal(db, source, cfg, cancel, Some(identity_store)).await
}
