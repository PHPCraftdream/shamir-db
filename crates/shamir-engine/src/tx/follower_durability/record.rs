use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_storage::types::Store;

use crate::meta::repl_bookmark::{save_bookmark_and_clean_horizon, save_replication_bookmark};

/// Store a follower-local horizon together with its upstream bookmark.
pub(crate) async fn persist_batch_record(
    info: &Arc<dyn Store>,
    bookmark: u64,
    local_end: u64,
    journal_clean: bool,
) -> DbResult<()> {
    if journal_clean {
        save_bookmark_and_clean_horizon(info, bookmark, local_end).await
    } else {
        save_replication_bookmark(info, bookmark).await
    }
}
