use std::sync::Arc;

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;

use super::super::{reproject_for_downstream, PreparedEvent};
use super::durable_journal_barrier;
use crate::meta::repl_bookmark::{save_bookmark_and_clean_horizon, save_replication_bookmark};
use crate::repo::RepoInstance;

/// Store a follower-local horizon together with its upstream bookmark.
pub(super) async fn persist_batch_record(
    info: &Arc<dyn Store>,
    bookmark: u64,
    local_end: u64,
    journal_clean: bool,
) -> DbResult<()> {
    if journal_clean {
        save_bookmark_and_clean_horizon(info, bookmark, local_end).await
    } else {
        save_replication_bookmark(info, bookmark).await
    }
}

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
    let journal_clean = if bypass_barrier {
        true
    } else {
        durable_journal_barrier(repo, &events, start.unwrap_or(floor), end).await?
    };
    #[cfg(not(test))]
    let journal_clean = durable_journal_barrier(repo, &events, start.unwrap_or(floor), end).await?;
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
    persist_batch_record(info, leader_floor, end, journal_clean).await?;
    #[cfg(test)]
    super::test_seam::phase(repo.name(), super::test_seam::Phase::BookmarkPersisted).await;
    Ok(end)
}
