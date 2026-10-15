//! Result type of [`super::RepoChangefeed::read_from`] and the resumable
//! pull wrapper that produces it.

use std::sync::Arc;

use super::{ChangelogEvent, ChangelogStore, RepoChangefeed};

/// Result of a [`RepoChangefeed::read_from`] call.
///
/// `gap_at` is `Some(max(from_version, first_missing))` while the cursor is
/// at or before the invalidation horizon. The consumer must snapshot resync
/// rather than trusting the journal for an unbroken history.
pub struct JournalRead {
    /// Events read from the journal.
    pub events: Vec<ChangelogEvent>,
    /// `Some(v)` if a gap is known at version `v >= from_version`.
    pub gap_at: Option<u64>,
}

impl RepoChangefeed {
    /// Read up to `limit` journal events with `commit_version >=
    /// from_version`, ascending. Resumable pull: a consumer that processed
    /// through version V calls `read_from(V + 1, n)` to continue.
    ///
    /// Returns a [`JournalRead`] that carries the events AND a `gap_at` field.
    /// When `gap_at` is `Some(v)` a known-dropped version `v >= from_version`
    /// exists — the journal is not contiguous from `from_version` and the
    /// consumer must perform a full snapshot resync instead of trusting it for
    /// an unbroken history.
    pub async fn read_from(
        &self,
        store: &Arc<dyn ChangelogStore>,
        from_version: u64,
        limit: usize,
    ) -> JournalRead {
        self.read_from_bounded(store, from_version, limit, u64::MAX)
            .await
    }
}
