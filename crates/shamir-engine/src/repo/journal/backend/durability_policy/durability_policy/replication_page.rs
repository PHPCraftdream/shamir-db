//! Replication-only journal clamp; ordinary changefeed reads retain W.

use shamir_query_types::ReplicationServe;
use shamir_storage::error::DbResult;

use super::super::{replication_bound_observed, DurabilityMetrics, REPLICATION_BOUND_WAIT};
use crate::repo::{BoundedJournalRead, RepoInstance};

pub(crate) async fn read_replication_page(
    repo: &RepoInstance,
    from_version: u64,
    limit: usize,
    // Mode snapshot taken ONCE upstream in `read_changelog_replication_page`
    // (before any `.await`, out of the arc_swap Guard) — no second load here.
    mode: ReplicationServe,
    metrics: &DurabilityMetrics,
) -> DbResult<BoundedJournalRead> {
    let mut first = repo
        .read_changelog_bounded_impl(from_version, limit, false, None)
        .await?;
    let group = repo.wal_group().await?;
    // Negative control removes the complete page clamp, metadata included.
    #[cfg(test)]
    if super::super::replication_bound::SKIP_S_CLAMP_FOR_TEST
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Ok(first);
    }
    // PR8 chunk 3 (`replication_serve = completed_reseed`): the serve bound is
    // `min(confirmed, W)` — exactly what `read_changelog_bounded_impl` already
    // applied — with NO recoverability clamp `R`. Speculative serving past `R`
    // is legal ONLY because the lifecycle record is armed BEFORE this call
    // (chunk 2) and an unclean restart bumps `history_epoch`, so a follower's
    // bookmark for an unsynced version is fenced and forces a re-seed. The
    // R-bound observation (coalesced fsync + bounded wait) is skipped
    // entirely: raising `R` serves nobody in this mode. Metrics stay truthful
    // with the existing sample fields (no new snapshot arrays): `served`
    // equals the bound, because nothing is held back.
    if mode == ReplicationServe::CompletedReseed {
        metrics.observe_bound(first.bound, first.bound);
        return Ok(first);
    }
    // A full/short nonempty page needs its own maximum, not an unrelated
    // newer journal tail. Empty pages still require the full exhaustion bound.
    let target = first
        .read
        .events
        .iter()
        .map(|e| e.commit_version)
        .max()
        .map_or(first.bound, |version| {
            version.max(first.read.gap_at.unwrap_or(0))
        });
    let covered = replication_bound_observed(&group, target, REPLICATION_BOUND_WAIT, metrics).await;
    let bound = first.bound.min(group.recoverable_through());
    if covered >= target {
        first.bound = bound;
        first.confirmed = bound >= first.sampled_w;
        return Ok(first);
    }
    // Re-read once: the initial journal page may contain events above R.
    repo.read_changelog_bounded_impl(from_version, limit, false, Some(bound))
        .await
}
