use std::sync::Arc;

use shamir_tx::{ChangelogEvent, RepoChangefeed, VersionGuard};

use crate::repo::RepoInstance;

/// Publish only at the point of no return, after WAL success and the last
/// crash/failure exit. No event is ever enqueued for a version that never
/// publishes: preparation alone has no journal/gap side effect, and this
/// synchronous enqueue-or-gap + commit pair has no await or fallible step
/// between them. Cancellation cannot split the pair. Full/Closed mark a gap
/// in `journal_enqueue`; failed initialization records its gap here, not at
/// preparation time. The returned Arc is the SAME allocation used by the
/// channel and later live broadcast (one allocation, as in the old emit).
#[inline]
pub(super) fn publish_journaled(
    repo: &RepoInstance,
    guard: VersionGuard,
    prepared: Option<(Arc<ChangelogEvent>, Option<Arc<RepoChangefeed>>)>,
) -> Option<Arc<ChangelogEvent>> {
    let event = prepared.map(|(event, feed)| {
        match feed {
            Some(feed) => feed.journal_enqueue(&event),
            None => repo.record_pending_changefeed_gap(event.commit_version),
        }
        event
    });
    guard.commit();
    event
}
