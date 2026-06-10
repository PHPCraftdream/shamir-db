use super::ShamirDb;

impl ShamirDb {
    // ============================================================================
    // Changefeed (Phase 3b): live broadcast + durable journal
    // ============================================================================

    /// Subscribe to a repo's live changefeed (Phase 3b).
    ///
    /// Returns `None` when the database or repository does not exist. The
    /// returned `broadcast::Receiver` yields every `ChangelogEvent` emitted
    /// after the call; a subscriber that lags the bounded ring receives
    /// `RecvError::Lagged` and should re-sync the missed window via
    /// [`read_changelog_from`](Self::read_changelog_from).
    pub async fn subscribe_changelog(
        &self,
        db: &str,
        repo: &str,
    ) -> Option<tokio::sync::broadcast::Receiver<std::sync::Arc<shamir_engine::ChangelogEvent>>>
    {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        repo_instance.subscribe_changelog().await.ok()
    }

    /// Resumable pull from a repo's durable changelog journal (Phase 3b).
    ///
    /// Returns up to `limit` events with `commit_version >= from_version`,
    /// ascending, or `None` when the database / repository does not exist.
    /// A consumer that processed through version `V` continues with
    /// `read_changelog_from(db, repo, V + 1, n)`.
    ///
    /// If the journal has a known gap at or after `from_version` (due to a
    /// prior channel-overflow drop) a `warn!` is emitted. Use
    /// [`read_changelog_from_journal`](Self::read_changelog_from_journal) when
    /// the caller needs to act on the gap signal programmatically.
    pub async fn read_changelog_from(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: usize,
    ) -> Option<Vec<shamir_engine::ChangelogEvent>> {
        let jr = self
            .read_changelog_from_journal(db, repo, from_version, limit)
            .await?;
        if let Some(gap) = jr.gap_at {
            log::warn!(
                "changefeed journal gap detected: db={db} repo={repo} \
                 from_version={from_version} gap_at={gap}; \
                 consumer should perform a full snapshot resync"
            );
        }
        Some(jr.events)
    }

    /// Like [`read_changelog_from`](Self::read_changelog_from) but returns the
    /// full [`shamir_engine::JournalRead`] so the caller can inspect `gap_at`
    /// and decide whether a snapshot resync is needed.
    pub async fn read_changelog_from_journal(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: usize,
    ) -> Option<shamir_engine::JournalRead> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        repo_instance
            .read_changelog_from(from_version, limit)
            .await
            .ok()
    }
}
