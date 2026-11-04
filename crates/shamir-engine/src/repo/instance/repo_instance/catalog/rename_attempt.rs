use super::rename_freeze::RenameFreeze;
use super::rename_in_doubt::RenameInDoubt;
use super::RepoInstance;
use crate::repo::RepoWritePermit;
use crate::table::TableManager;
use shamir_storage::error::{DbError, DbResult};

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttemptPhase {
    Preparing,
    Persisting,
}

/// Cancellation-safe ownership of a table rename attempt.
pub(crate) struct RenameAttempt<'p> {
    repo: &'p RepoInstance,
    _permit: &'p RepoWritePermit,
    from: String,
    to: String,
    source: TableManager,
    freeze: Option<RenameFreeze>,
    was_fenced: bool,
    phase: AttemptPhase,
    ended: bool,
}

impl<'p> RenameAttempt<'p> {
    pub(crate) fn new(
        repo: &'p RepoInstance,
        permit: &'p RepoWritePermit,
        from: &str,
        to: &str,
        source: TableManager,
        pair: Option<RenameInDoubt>,
    ) -> Self {
        let was_fenced = pair.is_some();
        let (source, freeze) = match pair {
            Some(pair) => (pair.freeze.source().clone(), Some(pair.freeze)),
            None => (source, None),
        };
        Self {
            repo,
            _permit: permit,
            from: from.into(),
            to: to.into(),
            source,
            freeze,
            was_fenced,
            phase: AttemptPhase::Preparing,
            ended: false,
        }
    }

    pub(crate) fn was_fenced(&self) -> bool {
        self.was_fenced
    }

    pub(crate) fn install_freeze(&mut self, freeze: RenameFreeze) {
        self.freeze = Some(freeze);
    }

    pub(crate) fn source(&self) -> &TableManager {
        &self.source
    }

    pub(crate) fn freeze(&self) -> DbResult<&RenameFreeze> {
        self.freeze
            .as_ref()
            .ok_or_else(|| DbError::Internal("rename freeze is not installed".into()))
    }

    pub(crate) fn mark_persisting(&mut self) {
        self.phase = AttemptPhase::Persisting;
    }

    pub(crate) fn commit(mut self) {
        if let Some(freeze) = self.freeze.take() {
            freeze.retire();
        }
        if self.was_fenced {
            self.repo.clear_rename_fence(&self.from, &self.to);
        }
        self.ended = true;
    }

    /// Reattach only what this attempt itself installed: a fresh attempt
    /// that never passed `RenameFreeze::begin` holds no freeze and must be a
    /// no-op with respect to reattach/freeze teardown. An attempt only
    /// releases what it attached.
    pub(crate) fn abort(mut self) {
        if self.was_fenced {
            self.fence_retained();
        } else if let Some(freeze) = self.freeze.take() {
            self.source.reattach_after_rename();
            drop(freeze);
        }
        self.ended = true;
    }

    pub(crate) fn into_doubt(mut self) {
        self.fence_retained();
        self.ended = true;
    }

    fn fence_retained(&mut self) {
        if let Some(freeze) = self.freeze.take() {
            self.repo.raise_rename_fence(RenameInDoubt {
                from: self.from.clone(),
                to: self.to.clone(),
                freeze,
            });
        }
    }
}

impl Drop for RenameAttempt<'_> {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        if self.was_fenced || self.phase == AttemptPhase::Persisting {
            self.fence_retained();
        } else if let Some(freeze) = self.freeze.take() {
            // Same ownership rule as `abort`: only release a freeze this
            // attempt installed; a fresh attempt aborted before `begin`
            // succeeded holds none and must not reattach.
            self.source.reattach_after_rename();
            drop(freeze);
        }
    }
}
