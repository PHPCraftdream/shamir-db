use shamir_storage::error::{DbError, DbResult};

use super::{VisibilityPendingReason, VisibilityState};

/// A known direct write, not a transactional atomicity or fsync guarantee.
#[must_use]
#[derive(Debug)]
pub struct WriteOutcome<T> {
    pub value: T,
    pub commit_version: Option<u64>,
    pub visibility: VisibilityState,
}

impl<T> WriteOutcome<T> {
    pub fn into_strict_value(self) -> DbResult<T> {
        match self.visibility {
            VisibilityState::Visible => Ok(self.value),
            VisibilityState::Pending {
                visible_through,
                reason,
                ..
            } => Err(DbError::WriteCommittedVisibilityPending {
                commit_version: self.commit_version.expect("pending write minted a version"),
                visible_through,
                reason: match reason {
                    VisibilityPendingReason::Timeout => "timeout",
                    VisibilityPendingReason::ReopenRequired => "reopen_required",
                },
            }),
        }
    }
}
