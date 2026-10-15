//! Activation rule (PR6b-2): the receipt protocol applies to a WAL-less
//! follower batch when the backend supports repository-wide atomic batches
//! AND the stored bookmark is v3 or ABSENT (freshly installed by this
//! version). A v1/v2 bookmark keeps the legacy apply path until PR6b-3
//! migrates it. A backend without the capability is rejected with
//! `DbError::AtomicBatchUnsupported` where the new protocol is required.

use std::sync::Arc;

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

use crate::repo::RepoInstance;

/// Negative-control seam: forces the legacy (unprotected) apply path so the
/// crash-harness tests can contrast it with the receipt path.
#[cfg(test)]
static RECEIPT_LOOKUP_DISABLED: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
#[allow(dead_code)] // driven by the PR6b-2 test chunk
pub(crate) fn disable_receipt_lookup(disabled: bool) {
    RECEIPT_LOOKUP_DISABLED.store(disabled, Ordering::Release);
}

pub(crate) async fn receipt_path_selected(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
) -> DbResult<bool> {
    #[cfg(test)]
    if RECEIPT_LOOKUP_DISABLED.load(Ordering::Acquire) {
        return Ok(false);
    }
    let v3 = crate::meta::repl::bookmark_v3::has_v3_record(info).await?;
    if !v3
        && crate::meta::repl::bookmark_v3::load_bookmark_v3(info)
            .await?
            .is_some()
    {
        // A v1/v2 record is present: legacy path until PR6b-3 migrates it.
        return Ok(false);
    }
    // Fresh install: both checks above found nothing.
    if !repo.storage_supports_atomic_batch() {
        return Err(DbError::AtomicBatchUnsupported(format!(
            "follower repo {} requires the receipt apply protocol but its \
             backend does not support repository-wide atomic batches",
            repo.name()
        )));
    }
    Ok(true)
}
