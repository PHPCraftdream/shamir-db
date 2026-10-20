//! Ambient interner epoch-delta sync (Stage 5-wire Part A).
//!
//! After executing a batch, the server inspects the client's advertised
//! per-repo epochs (`request.interner_epochs`) and attaches the server's
//! delta (`response.interner_delta`) for each repo. This mirrors the proven
//! `entries_after` usage in `admin_interner.rs:76-83`.
//!
//! The delta is always attached (even when empty) so the client can advance
//! its epoch to the server's high-water mark; `skip_serializing_if` keeps the
//! wire clean for empty cases. Unknown repos are silently skipped.

use crate::engine::db_instance::db_instance::DbInstance;
use crate::query::batch::{BatchError, BatchRequest, BatchResponse, InternerDelta};

/// Populate `response.interner_delta` from the server's per-repo interners.
///
/// For each `(repo, client_epoch)` in `request.interner_epochs`: resolve the
/// db's `RepoInstance`, `repo_interner().get().await`, `entries_after(epoch)`,
/// and build an [`InternerDelta`] from `(entries, new_high)`. Unknown repos are
/// skipped. Errors loading an interner are surfaced as a soft `BatchError` —
/// the batch itself already executed successfully.
pub(super) async fn attach_interner_delta(
    response: &mut BatchResponse,
    request: &BatchRequest,
    db: &DbInstance,
) -> Result<(), BatchError> {
    if request.interner_epochs.is_empty() {
        return Ok(());
    }
    for (repo_name, client_epoch) in &request.interner_epochs {
        let Some(repo) = db.get_repo(repo_name) else {
            // Unknown repo → skip (the batch may have created/dropped it).
            continue;
        };
        // Load the per-repo interner. A failure here means the interner store
        // could not be opened; we skip this repo rather than failing the whole
        // response — the batch results are already computed.
        let mgr = match repo.repo_interner().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let interner = match mgr.get().await {
            Ok(i) => i,
            Err(_) => continue,
        };
        let (entries, new_high) = interner.entries_after(*client_epoch as usize);
        let delta = InternerDelta {
            epoch: new_high as u64,
            entries: entries
                .into_iter()
                .map(|(k, u)| (k.id(), u.as_str().to_owned()))
                .collect(),
        };
        response.interner_delta.insert(repo_name.clone(), delta);
    }
    Ok(())
}
