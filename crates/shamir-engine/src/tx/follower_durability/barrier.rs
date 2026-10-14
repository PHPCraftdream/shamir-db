use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::ChangelogEvent;

use crate::repo::RepoInstance;

const CONFIRM_BUDGET: Duration = Duration::from_secs(1);

pub(crate) async fn single_barrier(
    repo: &RepoInstance,
    event: Option<&Arc<ChangelogEvent>>,
    version: u64,
) -> DbResult<()> {
    let events = event.cloned().into_iter().collect::<Vec<_>>();
    durable_journal_barrier(repo, &events, version, version)
        .await
        .map(|_| ())
}

/// Persisted gaps prevent a clean-prefix certification, but do not reject applying
/// an otherwise valid replicated batch. The returned flag permits clean-horizon advancement.
pub(crate) async fn durable_journal_barrier(
    repo: &RepoInstance,
    events: &[Arc<ChangelogEvent>],
    start: u64,
    end: u64,
) -> DbResult<bool> {
    let handle = repo.changefeed().await?;
    #[cfg(test)]
    if super::test_seam::barrier_action(repo.name()).0 {
        return Err(DbError::Storage(
            "injected follower journal barrier failure".into(),
        ));
    }
    let confirmed = handle.feed.confirm_through(end, CONFIRM_BUDGET).await;
    if confirmed < end {
        return Err(DbError::Storage(format!(
            "follower journal confirmation stopped at {confirmed}, required {end}"
        )));
    }

    #[cfg(test)]
    super::test_seam::phase(repo.name(), super::test_seam::Phase::JournalConfirmed).await;

    let store = repo.follower_journal_store().await?;
    let first_gap = handle.feed.first_gap_version();
    if first_gap != 0 && first_gap <= end {
        // The sole writer persists the merged runtime interval and flushes it.
        handle
            .feed
            .record_recovery_gap(first_gap, first_gap)
            .await?;
    }
    let gap = handle.feed.certified_gap_interval();
    let can_advance_horizon = gap.is_none_or(|(first, _)| first > end);
    verify_events_and_flush(store.as_ref(), events, start, end, gap).await?;
    Ok(can_advance_horizon)
}

/// Verify the journal read-back and flush it as one batch-level certification.
pub(crate) async fn verify_events_and_flush(
    store: &dyn Store,
    events: &[Arc<ChangelogEvent>],
    start: u64,
    end: u64,
    gap: Option<(u64, u64)>,
) -> DbResult<()> {
    verify_events(store, events, start, end, gap).await?;
    store.flush().await
}

#[cfg(test)]
pub(crate) async fn verify_events_without_flush(
    store: &dyn Store,
    events: &[Arc<ChangelogEvent>],
    start: u64,
    end: u64,
    gap: Option<(u64, u64)>,
) -> DbResult<()> {
    verify_events(store, events, start, end, gap).await
}

async fn verify_events(
    store: &dyn Store,
    events: &[Arc<ChangelogEvent>],
    start: u64,
    end: u64,
    gap: Option<(u64, u64)>,
) -> DbResult<()> {
    for event in events {
        let version = event.commit_version;
        if version < start || version > end {
            return Err(DbError::Storage(format!(
                "journal event version {version} is outside batch [{start}, {end}]"
            )));
        }
        if gap.is_some_and(|(first, through)| version >= first && version <= through) {
            continue;
        }
        let expected = serialize_event(event)?;
        let key = RecordKey::from_slice(&version.to_be_bytes());
        match store.get(key).await {
            Ok(actual) if actual == expected => {}
            Ok(_) => {
                return Err(DbError::Storage(format!(
                    "follower journal event mismatch at local version {version}"
                )));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn serialize_event(event: &ChangelogEvent) -> DbResult<Bytes> {
    rmp_serde::to_vec(event)
        .map(Bytes::from)
        .map_err(|error| DbError::Storage(format!("follower journal encode: {error}")))
}
