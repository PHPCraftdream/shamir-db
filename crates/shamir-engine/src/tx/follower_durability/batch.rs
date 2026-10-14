use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

use super::super::{reproject_for_downstream, PreparedEvent};
use super::{durable_journal_barrier, persist_batch_record};
use crate::repo::RepoInstance;

/// Finish one follower batch after its events have been applied and queued.
pub(in crate::tx::apply_replicated) async fn finish_batch(
    repo: &RepoInstance,
    info: &Arc<dyn Store>,
    prepared: &[PreparedEvent],
    floor: u64,
    leader_floor: u64,
) -> DbResult<u64> {
    let mut events = Vec::with_capacity(prepared.len());
    let mut start = None;
    let mut end = floor;
    for event in prepared {
        let version = event.version.ok_or_else(|| {
            DbError::Storage("replicated event lost its allocated local version".into())
        })?;
        start.get_or_insert(version);
        end = version;
        events.push(Arc::new(reproject_for_downstream(
            &event.event,
            repo.name(),
            version,
        )));
    }
    #[cfg(test)]
    super::test_seam::phase(
        repo.name(),
        super::test_seam::Phase::AfterPerEventHistoryWrites,
    )
    .await;
    #[cfg(test)]
    let (_, bypass_barrier) = super::test_seam::barrier_action(repo.name());
    #[cfg(test)]
    if !bypass_barrier {
        durable_journal_barrier(repo, &events, start.unwrap_or(floor), end).await?;
    }
    #[cfg(not(test))]
    durable_journal_barrier(repo, &events, start.unwrap_or(floor), end).await?;
    #[cfg(test)]
    if !bypass_barrier {
        super::test_seam::phase(repo.name(), super::test_seam::Phase::AfterJournalFlush).await;
    }
    #[cfg(test)]
    super::test_seam::phase(
        repo.name(),
        super::test_seam::Phase::BeforeCombinedHistoryFlush,
    )
    .await;
    #[cfg(test)]
    if super::test_seam::fail_history_flush(repo.name()) {
        return Err(DbError::Storage(
            "injected follower history flush failure".into(),
        ));
    }
    repo.flush_all_history().await?;
    let group = repo.wal_group().await?;
    let gate = repo.tx_gate().await?;
    for event in prepared {
        if let Some(version) = event.version {
            gate.mark_durable(version);
            group.settle(version);
        }
    }
    persist_batch_record(info, leader_floor, end).await?;
    #[cfg(test)]
    super::test_seam::phase(repo.name(), super::test_seam::Phase::BookmarkPersisted).await;
    Ok(end)
}
