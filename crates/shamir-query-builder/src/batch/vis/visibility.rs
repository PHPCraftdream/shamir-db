use shamir_query_types::batch::VisibilityWait;
use shamir_query_types::wire::{DbRequest, CURRENT_QUERY_LANG_VERSION};

/// Build a read-only settlement observation request (no transaction handle).
pub fn wait_for_visibility(
    db: impl Into<String>,
    repo: impl Into<String>,
    version: u64,
    visibility_wait: VisibilityWait,
) -> DbRequest {
    DbRequest::WaitForVisibility {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.into(),
        repo: repo.into(),
        version,
        visibility_wait,
    }
}
