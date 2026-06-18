use std::sync::Arc;

use super::ShamirDb;
use shamir_types::codecs::interned::json::inner_value_to_query_value;
use shamir_types::core::interner::Interner;
use shamir_types::types::value::{InnerValue, QueryValue};
use tokio::sync::OnceCell;

impl ShamirDb {
    /// Decode a changefeed `RecordChange.value` byte slice (MessagePack
    /// encoded `InnerValue` with interned `u64` map keys) back into a
    /// `QueryValue` with string keys, using the named table's interner for
    /// de-interning.
    ///
    /// Returns `None` when the database / repo / table doesn't exist,
    /// the interner can't be loaded, or the bytes don't decode as a
    /// valid `InnerValue` — callers treat `None` as fail-closed.
    ///
    /// For the hot subscription filter path prefer
    /// [`decode_record_value_inner`] + lazy conversion, which skips the
    /// de-intern cost for events that are filtered out before delivery.
    pub async fn decode_record_value_query_value(
        &self,
        db: &str,
        repo: &str,
        table: &str,
        bytes: &[u8],
    ) -> Option<QueryValue> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        let table_manager = repo_instance.get_table(table).await.ok()?;
        let interner = table_manager.interner().get().await.ok()?;
        let inner: InnerValue = rmp_serde::from_slice(bytes).ok()?;
        inner_value_to_query_value(&inner, interner).ok()
    }

    /// Decode a changefeed `RecordChange.value` byte slice into an `InnerValue`
    /// and return both the decoded value and a shared handle to the table's
    /// interner cell (needed to resolve interned field-name keys for filter
    /// evaluation without keeping the `TableManager` alive).
    ///
    /// After `interner_manager.get().await` completes (which this function does
    /// internally), the returned `Arc<OnceCell<Interner>>` is already populated.
    /// Callers can therefore call `cell.get().unwrap()` synchronously for filter
    /// evaluation without any additional async overhead.
    ///
    /// Cheaper than [`decode_record_value_query_value`] for the filter-only
    /// path: it skips the interner reverse-lookup and `QueryValue` allocation
    /// entirely, paying only the `rmp_serde::from_slice` decode.  The caller
    /// converts to `QueryValue` lazily only when the event passes the filter
    /// and must be delivered.
    pub async fn decode_record_value_inner(
        &self,
        db: &str,
        repo: &str,
        table: &str,
        bytes: &[u8],
    ) -> Option<(InnerValue, Arc<OnceCell<Interner>>)> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        let table_manager = repo_instance.get_table(table).await.ok()?;
        // Initialize the interner (no-op if already warm) and grab a
        // shared handle to the OnceCell so the decode cache can hold it
        // without keeping the TableManager alive.
        let _ = table_manager.interner().get().await.ok()?;
        let interner_cell = table_manager.interner().interner_cell();
        let inner: InnerValue = rmp_serde::from_slice(bytes).ok()?;
        Some((inner, interner_cell))
    }

    /// Return the table's shared `Arc<OnceCell<Interner>>` (guaranteed populated
    /// after this call) without decoding any record bytes. Used by the
    /// subscription bridge to cache raw bytes + interner instead of a decoded
    /// `InnerValue` tree.
    ///
    /// Returns `None` when the database / repo / table doesn't exist or the
    /// interner can't be loaded.
    pub async fn get_table_interner_cell(
        &self,
        db: &str,
        repo: &str,
        table: &str,
    ) -> Option<Arc<OnceCell<Interner>>> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        let table_manager = repo_instance.get_table(table).await.ok()?;
        let _ = table_manager.interner().get().await.ok()?;
        Some(table_manager.interner().interner_cell())
    }

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

    /// Returns the current (last committed) version for a repo.
    ///
    /// Returns `None` when the database or repository does not exist, or
    /// the tx gate has not been initialised yet. Used by the subscription
    /// bridge to seed watermarks after an initial snapshot.
    pub async fn current_commit_version(&self, db: &str, repo: &str) -> Option<u64> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        repo_instance.current_commit_version().await.ok()
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
