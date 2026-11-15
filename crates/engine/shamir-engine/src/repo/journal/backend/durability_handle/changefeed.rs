//! Changefeed handle plus publish-fallback and bounded journal read core.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use shamir_storage::error::{DbError, DbResult};

use super::DurabilityHandle;

/// Fixed read-side budget, not a tunable: a stalled writer returns an older
/// safe journal prefix after at most one second of admission + confirmation.
/// Barriers share the bounded writer channel's slots with journal events.
const JOURNAL_CONFIRM_BUDGET: Duration = Duration::from_secs(1);

/// Bundle of the per-repo changefeed and the store it journals into.
/// The store is retained so the shell's `read_changelog_from` can
/// range-read the journal without re-resolving it.
#[derive(Clone)]
pub(crate) struct ChangefeedHandle {
    pub(crate) feed: Arc<shamir_tx::RepoChangefeed>,
    pub(crate) store: Arc<dyn shamir_tx::ChangelogStore>,
}

impl DurabilityHandle {
    /// Synchronous publish-time fallback; minimum is shared across repo clones.
    pub(crate) fn record_pending_changefeed_gap(&self, version: u64) {
        let version = version.max(1);
        let _ = self.pending_changefeed_gap.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |current| {
                Some(if current == 0 {
                    version
                } else {
                    current.min(version)
                })
            },
        );
        if let Some(handle) = self.changefeed.get() {
            handle.feed.mark_gap(version);
        }
    }

    /// Live only: never re-enqueue an event already handled before publish.
    pub(crate) fn broadcast_changefeed_event(&self, event: Option<Arc<shamir_tx::ChangelogEvent>>) {
        if let (Some(event), Some(handle)) = (event, self.changefeed.get()) {
            if handle.feed.subscriber_count() > 0 {
                handle.feed.broadcast(event);
            }
        }
    }

    pub(crate) fn journal_dropped_total(&self) -> u64 {
        self.changefeed
            .get()
            .map_or(0, |handle| handle.feed.journal_dropped())
    }

    /// Core of the shell's bounded journal read, after the two resolutions
    /// (gate sampling and changefeed init). Store range failures are errors,
    /// not empty pages; corruption and known holes retain the journal's
    /// `gap_at` semantics.
    /// cancel-safe: yes — confirmation advances only a monotonic prefix, and
    /// the single range read mutates no records.
    pub(crate) async fn read_changelog_bounded_from(
        &self,
        h: ChangefeedHandle,
        sampled_w: u64,
        from_version: u64,
        limit: usize,
        preserve_errors: bool,
        // Replication-only extra clamp (`R`); `None` everywhere else —
        // changefeed subscribers keep the completion-watermark bound.
        r_clamp: Option<u64>,
    ) -> DbResult<crate::repo::BoundedJournalRead> {
        let bound = h
            .feed
            .confirm_through(sampled_w, JOURNAL_CONFIRM_BUDGET)
            .await
            .min(sampled_w)
            .min(r_clamp.unwrap_or(u64::MAX));
        let observed = preserve_errors.then(|| {
            Arc::new(
                crate::repo::bounded_journal_read::ErrorPreservingJournalStore::new(
                    h.store.clone(),
                ),
            )
        });
        let store: Arc<dyn shamir_tx::ChangelogStore> = match &observed {
            Some(store) => store.clone(),
            None => h.store,
        };
        let read = h
            .feed
            .read_from_bounded(&store, from_version, limit, bound)
            .await;
        if let Some(observed) = observed {
            if let Some(error) = observed.error() {
                return Err(DbError::Storage(error.to_owned()));
            }
        }
        Ok(crate::repo::BoundedJournalRead {
            read,
            sampled_w,
            bound,
            confirmed: bound >= sampled_w,
        })
    }
}
