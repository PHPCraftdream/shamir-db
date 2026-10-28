use shamir_query_types::batch::{VisibilityPendingReason, VisibilityState};
use shamir_storage::error::DbError;

use crate::table::WriteOutcome;

#[test]
fn pending_visibility_without_commit_version_returns_internal_error() {
    let error = WriteOutcome {
        value: (),
        commit_version: None,
        visibility: VisibilityState::Pending {
            visible_through: 0,
            blocking_version: 1,
            reason: VisibilityPendingReason::Timeout,
        },
    }
    .into_strict_value()
    .unwrap_err();

    assert!(matches!(
        error,
        DbError::Internal(message) if message == "pending visibility must have a commit version"
    ));
}
