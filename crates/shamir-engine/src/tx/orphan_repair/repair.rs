use std::sync::Arc;

use crate::repo::RepoInstance;
use shamir_storage::error::{DbError, DbResult};
use shamir_wal::WalEntryV2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tx::drainer) enum Outcome {
    Repaired,
    Gap(GapReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::tx::drainer) enum GapReason {
    UnknownWAL,
    EnqueueFailed,
    WriterGap,
}

impl std::fmt::Display for GapReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownWAL => f.write_str("unknown_or_eventless_wal"),
            Self::EnqueueFailed => f.write_str("journal_writer_unavailable"),
            Self::WriterGap => f.write_str("journal_enqueue_or_persist_failed"),
        }
    }
}

/// WAL-derived event repair. `Outcome::Gap` denotes conservative fallback;
/// callers must mark that GAP before settling the orphan.
///
/// The replay guard serializes retries. FIFO clean-horizon acknowledgements
/// drain the previous prefix even if confirm_through's watermark fast path
/// would return. They only record the current safe J and may fsync the journal;
/// the commit path is unchanged.
pub(in crate::tx::drainer) async fn repair_event(
    repo: &RepoInstance,
    entry: &WalEntryV2,
) -> DbResult<Outcome> {
    let version = entry.commit_version;
    let Some(event) = shamir_tx::event_from_wal_entry(entry) else {
        return Ok(Outcome::Gap(GapReason::UnknownWAL));
    };
    let handle = match repo.changefeed().await {
        Ok(handle) => handle,
        Err(error) => {
            log::warn!("orphan {version}: changefeed unavailable: {error}");
            return Ok(Outcome::Gap(GapReason::EnqueueFailed));
        }
    };
    let feed = &handle.feed;
    let safe_horizon = feed.clean_horizon();
    if safe_horizon >= version {
        return Err(DbError::Internal(format!(
            "orphan {version} is already covered by clean horizon {safe_horizon}"
        )));
    }

    // This ack is always queued, even if the confirmed watermark already
    // reaches the version. It settles any timed-out earlier enqueue before
    // the idempotence lookup.
    if let Err(error) = feed.record_clean_horizon(safe_horizon).await {
        // A closed writer cannot acknowledge the prefix; return the conservative
        // fallback so the caller marks a GAP instead of stranding the orphan.
        log::warn!("orphan {version}: preflight writer barrier failed: {error}");
        return Ok(Outcome::Gap(GapReason::EnqueueFailed));
    }

    let key = shamir_tx::changefeed::version_key(version);
    let rows = handle
        .store
        .range_from(key.clone(), 1)
        .await
        .map_err(DbError::Storage)?;
    if let Some(bytes) = rows.first() {
        let found = rmp_serde::from_slice::<shamir_tx::ChangelogEvent>(bytes)
            .map_err(|error| DbError::Codec(format!("orphan {version} event decode: {error}")))?;
        if let Some(expected) = shamir_tx::event_from_wal_entry(entry) {
            if found.commit_version == version {
                if found == expected {
                    return Ok(Outcome::Repaired);
                }
                return Err(DbError::Internal(format!(
                    "orphan {version} existing journal event differs from WAL"
                )));
            }
        }
    }

    let event = Arc::new(event);
    feed.journal_enqueue(&event);
    if let Err(error) = feed.record_clean_horizon(safe_horizon).await {
        // This command's failed send means the writer is closed. Event enqueue
        // also marks the version gap when the channel is closed/full.
        log::warn!("orphan {version}: journal confirmation failed: {error}");
        return Ok(Outcome::Gap(GapReason::EnqueueFailed));
    }

    let rows = handle
        .store
        .range_from(key, 1)
        .await
        .map_err(DbError::Storage)?;
    let Some(bytes) = rows.first() else {
        return Ok(Outcome::Gap(GapReason::WriterGap));
    };
    let found = rmp_serde::from_slice::<shamir_tx::ChangelogEvent>(bytes).map_err(|error| {
        DbError::Codec(format!("orphan {version} persisted event decode: {error}"))
    })?;
    if found.commit_version != version {
        return Ok(Outcome::Gap(GapReason::WriterGap));
    }
    if found == *event {
        Ok(Outcome::Repaired)
    } else {
        Err(DbError::Internal(format!(
            "orphan {version} persisted journal event differs from WAL"
        )))
    }
}
