use std::sync::Arc;

use super::ShamirDb;
// S5c-X F1 (Round 6): master's PR8b decode path is ported to this branch's
// boundary — decoding goes through `InternerReadView::inner_to_query_value`
// instead of the free `inner_value_to_query_value` over a raw `&Interner`,
// so the raw `Interner` type never crosses the crate boundary.
use crate::shamir_db::execute::skip_read_quarantine_for_test;
use crate::shamir_db::ChangelogReceiver;
use shamir_storage::error::DbError;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::engine::table::InternerReadView;

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
        let interner = table_manager
            .info_store_read_view()
            .interner_read_view()
            .await
            .ok()?;
        let inner: InnerValue = rmp_serde::from_slice(bytes).ok()?;
        // S5c-X F1 (Round 6): the de-intern runs on the engine-owned
        // read-only view — the raw `&Interner` never crosses the boundary.
        interner.inner_to_query_value(&inner).ok()
    }

    /// Decode a changefeed `RecordChange.value` byte slice into an `InnerValue`
    /// and return both the decoded value and a shared READ-ONLY handle to the
    /// table's interner dictionary (needed to resolve interned field-name keys
    /// for filter evaluation without keeping the `TableManager` alive).
    ///
    /// The returned `Arc<InternerReadView>` is guaranteed populated — callers
    /// evaluate filters synchronously through its read accessors (S5c-X F1,
    /// Round 6: the raw `Arc<OnceCell<Interner>>` no longer crosses the
    /// boundary).
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
    ) -> Option<(InnerValue, Arc<InternerReadView>)> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        let table_manager = repo_instance.get_table(table).await.ok()?;
        // Initialize the interner (no-op if already warm) and grab a
        // shared read-only handle so the decode cache can hold it
        // without keeping the TableManager alive.
        let interner = table_manager
            .info_store_read_view()
            .interner_read_view()
            .await
            .ok()?;
        let inner: InnerValue = rmp_serde::from_slice(bytes).ok()?;
        Some((inner, interner))
    }

    /// Return the table's shared read-only interner handle (guaranteed
    /// populated after this call) without decoding any record bytes. Used by
    /// the subscription bridge to cache raw bytes + interner instead of a
    /// decoded `InnerValue` tree.
    ///
    /// Returns `None` when the database / repo / table doesn't exist or the
    /// interner can't be loaded.
    pub async fn get_table_interner_read_view(
        &self,
        db: &str,
        repo: &str,
        table: &str,
    ) -> Option<Arc<InternerReadView>> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        let table_manager = repo_instance.get_table(table).await.ok()?;
        table_manager
            .info_store_read_view()
            .interner_read_view()
            .await
            .ok()
    }

    // ============================================================================
    // Changefeed (Phase 3b): live broadcast + durable journal
    // ============================================================================

    /// Subscribe to a repo's live changefeed (Phase 3b).
    ///
    /// Returns `Ok(None)` when the database or repository does not exist. The
    /// returned revocable receiver reports Tokio-compatible `Lagged`/`Closed`
    /// errors. Quarantine discards queued events and revokes attached receivers;
    /// use its delivery gate to guard synchronous downstream acceptance.
    ///
    /// S5c-X PR8b: a quarantined (`resync_required`) replication destination
    /// is refused before resolving the broadcast; the live receiver also
    /// closes on transition. A raw Tokio receiver is never exposed.
    pub async fn subscribe_changelog(
        &self,
        db: &str,
        repo: &str,
    ) -> shamir_storage::error::DbResult<Option<ChangelogReceiver>> {
        // S5c-X PR8b: refuse a quarantined repo before resolving the
        // broadcast (fail closed). Negative-control seam, test builds only —
        // mirrors `read_changelog_bounded` above.
        let skip_seam = skip_read_quarantine_for_test();
        if !skip_seam {
            if let Some(reason) = self.quarantined_read_reason(db, repo) {
                return Err(DbError::resync_required(db, repo, reason));
            }
        }
        let Some(repo_instance) = self.get_db(db).and_then(|d| d.get_repo(repo)) else {
            return Ok(None);
        };
        let _control = self.lock_replication_control().await;
        let receiver = repo_instance.subscribe_changelog().await.ok();
        let wrapped = receiver.map(|receiver| {
            super::revocation::receiver::ChangelogReceiver::new(
                receiver,
                (*self.changelog_receivers).clone(),
                db.to_string(),
                repo.to_string(),
            )
        });
        if let Some(wrapped) = &wrapped {
            self.changelog_receivers
                .register(db, repo, wrapped.delivery_gate());
            if self.quarantined_read_reason(db, repo).is_some() {
                wrapped.delivery_gate().close();
            }
        }
        drop(_control);
        if let Some(wrapped) = wrapped {
            Ok(Some(wrapped))
        } else {
            Ok(None)
        }
    }

    /// Resumable pull from a repo's durable changelog journal (Phase 3b).
    ///
    /// Returns up to `limit` events with `commit_version >= from_version`,
    /// ascending, or `Ok(None)` when the database / repository does not exist.
    /// A consumer that processed through version `V` continues with
    /// `read_changelog_from(db, repo, V + 1, n)`.
    ///
    /// S5c-X PR8b: a quarantined (`resync_required`) replication destination
    /// refuses the read with `DbError::ResyncRequired` (fail closed) — the
    /// consumer must reseed instead of consuming stale journal data.
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
    ) -> shamir_storage::error::DbResult<Option<Vec<shamir_engine::ChangelogEvent>>> {
        let Some(jr) = self
            .read_changelog_from_journal(db, repo, from_version, limit)
            .await?
        else {
            return Ok(None);
        };
        if let Some(gap) = jr.gap_at {
            log::warn!(
                "changefeed journal gap detected: db={db} repo={repo} \
                 from_version={from_version} gap_at={gap}; \
                 consumer should perform a full snapshot resync"
            );
        }
        Ok(Some(jr.events))
    }

    /// Returns the eager latest-state publication floor for a repo.
    ///
    /// Lower versions may still be unsettled. Use
    /// [`consistent_version`](Self::consistent_version) for snapshot seeds.
    /// Returns `None` when the database/repository is absent or gate init fails.
    pub async fn current_commit_version(&self, db: &str, repo: &str) -> Option<u64> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        repo_instance.current_commit_version().await.ok()
    }

    /// Returns the contiguous settled position for snapshot reads and seeds.
    /// Returns `None` when the database/repository is absent or gate init fails.
    pub async fn consistent_version(&self, db: &str, repo: &str) -> Option<u64> {
        let repo_instance = self.get_db(db)?.get_repo(repo)?;
        repo_instance.consistent_version().await.ok()
    }

    /// Pull-only journal read with field-name bindings and unchanged gap metadata.
    ///
    /// Only called from the follower's own pull/serve machinery. S5c-X PR8b:
    /// a quarantined (`resync_required`) destination is refused with
    /// `DbError::ResyncRequired` before any journal data is served (fail
    /// closed), same gate as
    /// [`read_changelog_bounded`](Self::read_changelog_bounded).
    pub async fn read_changelog_for_replication(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: usize,
    ) -> shamir_storage::error::DbResult<
        Option<(Vec<shamir_engine::tx::ReplicatedEvent>, Option<u64>)>,
    > {
        // S5c-X PR8b: refuse a quarantined repo before serving journal data
        // (fail closed). Negative-control seam, test builds only — mirrors
        // `read_changelog_bounded` below.
        let skip_seam = skip_read_quarantine_for_test();
        if !skip_seam {
            if let Some(reason) = self.quarantined_read_reason(db, repo) {
                return Err(DbError::resync_required(db, repo, reason));
            }
        }
        let Some(repo_instance) = self.get_db(db).and_then(|db| db.get_repo(repo)) else {
            return Ok(None);
        };
        repo_instance
            .read_changelog_for_replication(from_version, limit)
            .await
            .map(Some)
    }

    /// Completion cut for subscription activation, not last committed version.
    pub async fn changelog_visible_floor(&self, db: &str, repo: &str) -> Option<u64> {
        let repo = self.get_db(db)?.get_repo(repo)?;
        Some(repo.tx_gate().await.ok()?.visible_floor())
    }

    /// Error-preserving bounded journal page. `Ok(None)` means only that
    /// the database or repo does not exist; init/range failures remain errors.
    pub async fn read_changelog_bounded(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: usize,
    ) -> shamir_storage::error::DbResult<Option<shamir_engine::BoundedJournalRead>> {
        // S5c-X PR8b: a quarantined (`resync_required`) destination refuses
        // changelog pulls (fail closed) — the follower must reseed instead.
        // Negative-control seam is only compiled into test builds.
        let skip_seam = skip_read_quarantine_for_test();
        if !skip_seam {
            if let Some(reason) = self.quarantined_read_reason(db, repo) {
                return Err(DbError::resync_required(db, repo, reason));
            }
        }
        let Some(repo_instance) = self.get_db(db).and_then(|db| db.get_repo(repo)) else {
            return Ok(None);
        };
        repo_instance
            .read_changelog_bounded(from_version, limit)
            .await
            .map(Some)
    }

    /// Await completion progress, not journal persistence. Missing db/repo
    /// returns `Ok(None)`; reopening is reported in the visibility outcome.
    /// cancel-safe: yes — lookup owns its repo handle (no map guard), then
    /// delegates to the engine's cancellation-safe completion wait.
    pub async fn await_changelog_progress(
        &self,
        db: &str,
        repo: &str,
        target_version: u64,
    ) -> shamir_storage::error::DbResult<Option<shamir_engine::VisibleOutcome>> {
        let Some(repo_instance) = self.get_db(db).and_then(|db| db.get_repo(repo)) else {
            return Ok(None);
        };
        repo_instance
            .await_journal_progress(target_version)
            .await
            .map(Some)
    }

    /// Like [`read_changelog_from`](Self::read_changelog_from) but returns the
    /// full [`shamir_engine::JournalRead`] so the caller can inspect `gap_at`
    /// and decide whether a snapshot resync is needed. `Ok(None)` means only
    /// that the database or repo does not exist; a quarantined
    /// (`resync_required`) destination is refused with
    /// `DbError::ResyncRequired` (S5c-X PR8b, same gate as
    /// [`read_changelog_bounded`](Self::read_changelog_bounded)).
    pub async fn read_changelog_from_journal(
        &self,
        db: &str,
        repo: &str,
        from_version: u64,
        limit: usize,
    ) -> shamir_storage::error::DbResult<Option<shamir_engine::JournalRead>> {
        // S5c-X PR8b: a quarantined (`resync_required`) destination refuses
        // changelog reads (fail closed) — the follower must reseed instead.
        // Negative-control seam is only compiled into test builds.
        let skip_seam = skip_read_quarantine_for_test();
        if !skip_seam {
            if let Some(reason) = self.quarantined_read_reason(db, repo) {
                return Err(DbError::resync_required(db, repo, reason));
            }
        }
        let Some(repo_instance) = self.get_db(db).and_then(|db| db.get_repo(repo)) else {
            return Ok(None);
        };
        repo_instance
            .read_changelog_from(from_version, limit)
            .await
            .map(Some)
    }
}
