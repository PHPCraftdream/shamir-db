//! R1-b — durable per-(db,repo) follower replication bookmark.
//!
//! The bookmark is the highest LEADER `commit_version` the follower has
//! durably recorded as applied on this (db, repo). It is stored in the
//! repo's `__tx__` info-store under [`MetaKey::ReplicationBookmark`],
//! using the exact same [`MetaEnvelope`] codec as
//! [`LastCommittedVersion`](MetaKey::LastCommittedVersion) and
//! [`NextTxId`](MetaKey::NextTxId) (see `crate::meta::recovery_marker`).
//!
//! ## Why a separate marker from `LastCommittedVersion`
//!
//! `LastCommittedVersion` tracks the follower's LOCAL commit-version floor
//! (used by `RepoTxGate` to seed its monotonic counter on restart). The
//! replication bookmark tracks the LEADER's version space — a different
//! numbering domain (the follower allocates its own local version for each
//! applied event, per the R1-a version-allocation model). Conflating the two
//! would break either the gate's monotonicity invariant or the follower's
//! idempotency gating against the leader's sequence.
//!
//! ## Consumers
//!
//!  - [`crate::repo::RepoInstance::replication_bookmark`] /
//!    [`crate::repo::RepoInstance::advance_replication_bookmark`] — the
//!    repo-level wrappers a follower's pull-loop calls.
//!  - [`crate::tx::apply_replicated`] consumes the bookmark as its
//!    `applied_watermark` parameter (R1-a) for O(1) idempotent re-delivery
//!    gating.

use crate::meta::recovery_marker::{load_u64, save_u64};
use crate::meta::MetaKey;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use std::sync::Arc;

/// Load the durable replication bookmark from the info_store.
///
/// Returns `0` if the marker has never been written (fresh repo). Returns
/// `Err` only on real storage / decode failures. The `0` default matches
/// [`crate::tx::apply_replicated`]'s `applied_watermark` semantics: a fresh
/// follower applies every event whose `commit_version > 0` (i.e. every
/// real leader commit — version 0 is the bootstrap floor and never carries
/// a real event).
pub async fn load_replication_bookmark(info_store: &Arc<dyn Store>) -> DbResult<u64> {
    Ok(load_u64(info_store, MetaKey::ReplicationBookmark)
        .await?
        .unwrap_or(0))
}

/// Persist the replication bookmark.
///
/// This is the LOW-LEVEL store helper; callers that need monotonicity
/// protection against out-of-order delivery should prefer
/// [`crate::repo::RepoInstance::advance_replication_bookmark`], which
/// compares-then-swaps. Direct callers MUST ensure they never persist a
/// value smaller than the current one (the bookmark is a high-water mark,
/// not a free variable).
pub async fn save_replication_bookmark(info_store: &Arc<dyn Store>, version: u64) -> DbResult<()> {
    save_u64(info_store, MetaKey::ReplicationBookmark, version).await
}
