//! Sampled `visible_through` for `changes_since` responses.
//!
//! W is the settled prefix, sampled once BEFORE the journal read. It is a
//! settlement sample, not a coverage certificate: journal confirmation may lag
//! W and events stay subject to the cursor, limit, gaps and retention, so do
//! not advance the event cursor to W from this field alone. It bounds what a
//! caller may conclude: an empty event list with `visible_through` below the
//! caller's write position means the write is still pending, NOT that it
//! aborted or vanished. Journal gaps (`gap_at`) and retention still require
//! resynchronisation; this operation is not the primary reconciliation API
//! (`WaitForVisibility` is).

use shamir_engine::repo::RepoInstance;

/// Sample the settled prefix W of the repository. `None`
/// when the repository is not found or its gate is not initialized — the
/// field is then omitted from the response (additive, serde-defaulted).
pub(crate) fn changes_since_visible_through(repo: Option<&RepoInstance>) -> Option<u64> {
    repo.and_then(RepoInstance::visible_through_sample)
}
