use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use std::sync::Arc;

use crate::repo::RepoInstance;

/// Persist a recovery floor without allowing concurrent marker writers to regress it.
pub(crate) async fn reserve_recovery_floor(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
    minimum: u64,
) -> DbResult<u64> {
    repo.reserve_recovery_floor(info, minimum).await
}
