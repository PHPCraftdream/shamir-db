use super::ChangefeedHandle;
#[cfg(test)]
use crate::tx::clean_horizon::checkpoint::test_seam;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{Repo, Store};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::RepoInstance;
use super::JOURNAL_CONFIRM_BUDGET;

impl RepoInstance {
    /// Read the unchanged journal, attaching name bindings only for Pull.
    /// The tuple retains the journal's gap metadata; translation errors propagate.
    ///
    /// Replication-only R clamp; ordinary changefeeds retain completion W.
    pub async fn read_changelog_for_replication(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<(Vec<crate::tx::ReplicatedEvent>, Option<u64>)> {
        let journal = self
            .read_changelog_replication_page(from_version, limit)
            .await?
            .read;
        let manager = self.repo_interner().await?;
        let interner = manager.get().await?;
        let mut events = Vec::with_capacity(journal.events.len());
        for event in journal.events {
            let mut ids = shamir_collections::TFxMap::default();
            for change in &event.changes {
                if change.op == shamir_tx::ChangeOp::Put {
                    let bytes = change
                        .value
                        .as_ref()
                        .ok_or_else(|| DbError::Codec("replication Put has no value".into()))?;
                    let value = shamir_types::types::value::InnerValue::from_bytes(bytes)
                        .map_err(|error| DbError::Codec(error.to_string()))?;
                    shamir_tx::collect_referenced_ids(&value, &mut ids);
                }
            }
            let mut field_bindings = Vec::with_capacity(ids.len());
            for id in ids.keys() {
                let name = interner
                    .get_str(&shamir_types::core::interner::InternerKey::new(*id))
                    .ok_or_else(|| {
                        DbError::Codec(format!("replication field id {id} has no name"))
                    })?;
                field_bindings.push((*id, name.to_string()));
            }
            field_bindings.sort_unstable_by_key(|(id, _)| *id);
            events.push(crate::tx::ReplicatedEvent {
                event,
                field_bindings,
            });
        }
        Ok((events, journal.gap_at))
    }
    pub async fn certified_gap_interval(&self) -> DbResult<Option<(u64, u64)>> {
        Ok(self.changefeed().await?.feed.certified_gap_interval())
    }
    pub(crate) async fn mark_changefeed_gap(&self, version: u64) -> DbResult<()> {
        self.changefeed().await?.feed.mark_gap(version);
        Ok(())
    }

    // ============================================================================
    // Changefeed (Phase 3b): live broadcast + durable journal
    // ============================================================================

    /// cancel-safe: yes — `OnceCell::get_or_try_init`. If cancellation
    /// occurs during init the cell stays empty and the next caller retries;
    /// the returned handle is an in-memory `Arc` clone.
    ///
    /// Lazily build (once per repo) the changefeed: a live broadcast sender
    /// plus a background journal writer over the `"__changelog__"` store.
    /// The feed is "always on" — the commit-path feeds the journal on every
    /// non-empty commit regardless of live subscribers — so callers that
    /// subscribed late can still catch up via [`read_changelog_from`].
    ///
    /// [`read_changelog_from`]: Self::read_changelog_from
    pub(crate) async fn changefeed(&self) -> DbResult<ChangefeedHandle> {
        #[cfg(test)]
        test_seam::record(self, "before_feed_init");
        let handle = self
            .changefeed
            .get_or_try_init(|| async {
                #[cfg(test)]
                if let Some(hook) = self.journal_publish_hook.load_full() {
                    if hook
                        .init_failures
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                        .is_ok()
                    {
                        return Err(DbError::Storage("injected changefeed init failure".into()));
                    }
                }
                let store: Arc<dyn Store> = self.repo.store_get("__changelog__").await?;
                let adapter = crate::repo::changelog_store::StoreChangelog::new(store);
                let clean_horizon = adapter.load_clean_horizon().await?;
                let (marker, repair_marker) = adapter.load_gap_marker().await?;
                let cl_store: Arc<dyn shamir_tx::ChangelogStore> = Arc::new(adapter);
                let pending = self.pending_changefeed_gap.load(Ordering::Acquire);
                let restored = match (marker, pending) {
                    (m, 0) => m,
                    (None, p) => Some((p, p)),
                    (Some((first, through)), p) => Some((first.min(p), through.max(p))),
                };
                #[cfg(test)]
                if let Some(hook) = self.journal_publish_hook.load_full() {
                    if hook.init_snapshot_armed.swap(false, Ordering::AcqRel) {
                        hook.reached.notify_one();
                        hook.resume.notified().await;
                    }
                }
                // Seeds claim durability: flush pending/corrupt metadata before
                // spawning the sole writer. Failed init leaves pending intact.
                if repair_marker || restored != marker {
                    if let Some(interval) = restored {
                        cl_store
                            .put(
                                shamir_tx::changefeed::gap_marker_key(),
                                shamir_tx::changefeed::encode_gap_marker(interval),
                            )
                            .await
                            .map_err(DbError::Storage)?;
                    }
                }
                let feed = shamir_tx::RepoChangefeed::new_with_metadata(
                    Arc::clone(&cl_store),
                    restored,
                    clean_horizon,
                );
                Ok::<ChangefeedHandle, DbError>(ChangefeedHandle {
                    feed,
                    store: cl_store,
                })
            })
            .await?
            .clone();
        // Recheck AFTER OnceCell publishes, not only inside its closure: a failed
        // committer can store pending after the closure snapshot but before
        // get() becomes Some. Together with the publisher's store+get recheck,
        // either that publisher sees the feed or this path sees its minimum.
        // RMW (not just an Acquire load) participates in the pending atomic's
        // modification order. If the publisher's RMW is first, we see its gap;
        // if ours is first, its Acquire observes our Release and therefore the
        // already-published OnceCell. This closes the store-buffering schedule
        // without SeqCst or another lock, including publication after snapshot.
        let pending = self.pending_changefeed_gap.fetch_or(0, Ordering::AcqRel);
        if pending != 0 {
            handle.feed.mark_gap(pending);
        }
        Ok(handle)
    }

    /// Resolve before entering the synchronous publish block. Failure is
    /// logged, but does not mark a gap until this version actually publishes.
    pub(crate) async fn prepare_journal_event(
        &self,
        event: Option<shamir_tx::ChangelogEvent>,
    ) -> Option<(
        Arc<shamir_tx::ChangelogEvent>,
        Option<Arc<shamir_tx::RepoChangefeed>>,
    )> {
        let event = Arc::new(event?);
        let feed = match self.changefeed().await {
            Ok(handle) => Some(handle.feed),
            Err(e) => {
                log::warn!("changefeed init failed for repo {}: {e}", self.name());
                None
            }
        };
        Some((event, feed))
    }

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

    #[cfg(test)]
    pub(crate) async fn fire_post_publish_pre_broadcast_test_hook(&self) {
        if let Some(hook) = self.journal_publish_hook.load_full() {
            if hook.broadcast_armed.swap(false, Ordering::AcqRel) {
                hook.reached.notify_one();
                hook.resume.notified().await;
            }
        }
    }

    /// Subscribe to this repo's live changefeed.
    ///
    /// Returns a `broadcast::Receiver` that yields every `ChangelogEvent`
    /// emitted after the call. A subscriber that falls behind the bounded
    /// ring receives `RecvError::Lagged` and should re-sync the missed
    /// window via [`read_changelog_from`](Self::read_changelog_from).
    pub async fn subscribe_changelog(
        &self,
    ) -> DbResult<tokio::sync::broadcast::Receiver<Arc<shamir_tx::ChangelogEvent>>> {
        Ok(self.changefeed().await?.feed.subscribe())
    }

    /// Resumable pull: read up to `limit` durable journal events with
    /// `commit_version >= from_version`, ascending.
    ///
    /// Returns a [`shamir_tx::JournalRead`] which carries both the events and
    /// a `gap_at` field. When `gap_at` is `Some(v)` the journal has a known
    /// gap at version `v` (a prior overflow dropped that event) — callers
    /// should treat the feed as non-contiguous from `from_version`.
    /// The prefix is bounded by the contiguous completion watermark, not the
    /// potentially out-of-order last committed version. A stalled writer may
    /// return an older (possibly empty) prefix; confirmation logs/counts its
    /// own timeout, so this layer does not log a second warning.
    /// cancel-safe: yes — gate/feed initialization can be retried; confirmation
    /// only advances a monotonic prefix, and range reads mutate no data.
    pub async fn read_changelog_from(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<shamir_tx::JournalRead> {
        Ok(self
            .read_changelog_bounded_impl(from_version, limit, false, None)
            .await?
            .read)
    }

    /// Read a page and expose the actual confirmed bound, sampled before
    /// feed initialization. Store range failures are errors, not empty pages;
    /// corruption and known holes retain the journal's `gap_at` semantics.
    /// cancel-safe: yes — initialization is retryable, confirmation advances
    /// only a monotonic prefix, and the single range read mutates no records.
    pub async fn read_changelog_bounded(
        &self,
        from_version: u64,
        limit: usize,
    ) -> DbResult<crate::repo::BoundedJournalRead> {
        self.read_changelog_bounded_impl(from_version, limit, true, None)
            .await
    }

    pub(in crate::repo) async fn read_changelog_bounded_impl(
        &self,
        from_version: u64,
        limit: usize,
        preserve_errors: bool,
        // PR1: replication-only extra clamp (`R`). `None` everywhere else —
        // changefeed subscribers keep the completion-watermark bound.
        r_clamp: Option<u64>,
    ) -> DbResult<crate::repo::BoundedJournalRead> {
        let sampled_w = self.tx_gate().await?.completion().watermark();
        let h = self.changefeed().await?;
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

    /// Park until the completion prefix reaches `target_version`, including
    /// aborted/empty versions, or report that this gate requires reopening.
    /// This is completion progress, not journal persistence confirmation.
    /// cancel-safe: yes — retryable gate initialization followed by the
    /// gate's cancellation-safe notification wait; no added polling or state.
    pub async fn await_journal_progress(
        &self,
        target_version: u64,
    ) -> DbResult<shamir_tx::repo_tx_gate::VisibleOutcome> {
        Ok(self.tx_gate().await?.await_visible(target_version).await)
    }

    /// Persisted clean horizon J of the replication journal: every version
    /// through J is covered by the runtime clean checkpoint: journal events or persisted gaps.
    /// cancel-safe: yes — retryable lazy journal initialization, then an atomic read.
    pub async fn clean_horizon(&self) -> DbResult<u64> {
        Ok(self.changefeed().await?.feed.clean_horizon())
    }

    /// Stop the journal writer so the next clean-horizon checkpoint fails.
    /// Failure-injection seam for shutdown-reporting tests only.
    #[doc(hidden)]
    pub async fn stop_journal_writer_for_test(&self) -> DbResult<()> {
        self.changefeed().await?.feed.shutdown_and_wait().await
    }

    /// Install an isolated journal fixture before lazy initialization.
    #[cfg(test)]
    pub(crate) fn install_journal_for_test(&self, handle: ChangefeedHandle) {
        assert!(self.changefeed.set(handle).is_ok());
    }

    /// Test-only read-side observation of writer persistence/confirmation.
    #[cfg(test)]
    pub(crate) async fn journal_feed_for_test(&self) -> DbResult<Arc<shamir_tx::RepoChangefeed>> {
        Ok(self.changefeed().await?.feed)
    }
}
