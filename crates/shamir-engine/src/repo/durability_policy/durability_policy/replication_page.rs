//! Replication-only journal clamp; ordinary changefeed reads retain W.

use shamir_storage::error::DbResult;

use super::super::{replication_bound_observed, DurabilityMetrics, REPLICATION_BOUND_WAIT};
use crate::repo::{BoundedJournalRead, RepoInstance};

pub(crate) async fn read_replication_page(
    repo: &RepoInstance,
    from_version: u64,
    limit: usize,
    metrics: &DurabilityMetrics,
) -> DbResult<BoundedJournalRead> {
    let mut first = repo
        .read_changelog_bounded_impl(from_version, limit, false, None)
        .await?;
    let group = repo.wal_group().await?;
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
    // Negative control removes the complete page clamp, metadata included.
    #[cfg(test)]
    if super::super::replication_bound::SKIP_S_CLAMP_FOR_TEST
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Ok(first);
    }
    if covered >= target {
        first.bound = bound;
        first.confirmed = bound >= first.sampled_w;
        return Ok(first);
    }
    // Re-read once: the initial journal page may contain events above R.
    repo.read_changelog_bounded_impl(from_version, limit, false, Some(bound))
        .await
}
