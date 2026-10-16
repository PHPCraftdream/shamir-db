//! Follower admission (S5c-X PR6b-3): the receipt protocol is the ONLY
//! follower apply protocol. `admit_follower` validates the durable follower
//! state at first follower admission and returns the admission verdict;
//! unsupported durable states (legacy bookmark formats, malformed records,
//! floor-without-bookmark evidence, conflicting seed-install state) are
//! REFUSED with typed errors — there is no "choose legacy" result anymore
//! and no migration: the operator reseeds the affected subscription.
//! `Fresh` means the caller must ADOPT (write the v3 baseline) before any
//! apply work.

use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult, FollowerRefusalReason};
use shamir_storage::types::Store;

use crate::meta::repl::bookmark_v3::{classify_follower_bookmark, FollowerBookmarkState};
use crate::repo::RepoInstance;

/// Admission verdict for the first follower apply.
pub(crate) enum FollowerAdmission {
    /// A valid v3 bookmark record exists: durably identified follower.
    Native,
    /// No bookmark record and no floor/installation conflict: eligible —
    /// the caller must adopt (write the v3 baseline) before any apply work.
    Fresh,
}

/// Validate durable follower state at first follower admission. Typed
/// refusals propagate; `Fresh` means the caller must adopt (write the v3
/// baseline) before any apply work.
pub(crate) async fn admit_follower(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
) -> DbResult<FollowerAdmission> {
    match classify_follower_bookmark(info).await? {
        FollowerBookmarkState::Native(_) => Ok(FollowerAdmission::Native),
        FollowerBookmarkState::Fresh => {
            if crate::repo::seed_install::SeedInstallState::load(info)
                .await?
                .is_some()
            {
                // Conflicting installation state: a seed install is in
                // flight or incomplete — refusing is safer than adopting.
                return Err(DbError::FollowerStateRefused {
                    reason: FollowerRefusalReason::LegacyFollowerState,
                });
            }
            if !repo.storage_supports_atomic_batch() {
                return Err(DbError::AtomicBatchUnsupported(format!(
                    "follower repo {} requires the receipt apply protocol but its \
                     backend does not support repository-wide atomic batches",
                    repo.name()
                )));
            }
            Ok(FollowerAdmission::Fresh)
        }
        FollowerBookmarkState::FloorWithoutBookmark { .. } => {
            // A nonzero durable allocation floor without a v3 record is
            // unowned pre-receipt follower evidence: reseed required.
            Err(DbError::FollowerStateRefused {
                reason: FollowerRefusalReason::LegacyFollowerState,
            })
        }
    }
}
