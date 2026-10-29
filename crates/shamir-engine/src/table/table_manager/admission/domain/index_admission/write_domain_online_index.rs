use super::TableManager;
use crate::repo::RepoWritePermit;
use crate::table::table_manager::operations::table_manager_index_mgmt::PhaseBAResult;
use futures::StreamExt;
use shamir_index::base_index::index_keys::{build_index_key_from_record, build_posting_key};
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

impl TableManager {
    /// #1087: Phase B (micro-barrier) + Phase A (barrier-free backfill) for online CREATE INDEX.
    ///
    /// Performs the first two phases of online CREATE INDEX orchestration (RFC v2).
    /// Returns `Ok(true)` on success, `Ok(false)` if online build is unavailable
    /// (changefeed not wired), or `Err` on failure.
    ///
    /// Phase B: acquire barrier, open snapshot, register at Building, mark in-flight.
    /// Phase A: barrier-free scan via `mvcc_store().snapshot_stream()`, backfill postings.
    ///
    /// The index remains in `Building` state after this method returns (Phase C+D
    /// flip to Ready happens in #1088). The in-flight registry and dirty-set remain
    /// active, so concurrent writes during Phase A are captured.
    /// - `index_def`: Index definition with `state` already set to `Building` by the caller.
    /// - `batch_size`: Batch size for the `snapshot_stream` scan (same role as
    ///   `list_stream`'s parameter).
    ///
    /// #1089: Phase C (catch-up loop) + Phase D (publish barrier) for online CREATE INDEX.
    ///
    /// Phase B+A backfill (returns SnapshotGuard for Phase C/D use).
    ///
    /// Phase B holds the write barrier (`REGULAR_INDEX_CREATE`) across the snapshot
    /// pin and registration. Phase A runs barrier-free — the snapshot is versioned,
    /// so concurrent writes are visible to the dirty-set capture mechanism (activated
    /// by `mark_build_in_flight` in Phase B) but do not interfere with the scan.
    ///
    /// Returns `Ok(None)` when changefeed is not wired (online build unavailable).
    /// Returns `Ok(Some(PhaseBAResult { guard, pin }))` on success, where `guard`
    /// must be kept alive through Phase C/D (it pins the version for pin-time reads).
    pub(crate) async fn phase_b_a_backfill(
        &self,
        name: &str,
        index_def: crate::index::index_definition::IndexDefinition,
        batch_size: usize,
        permit: &RepoWritePermit,
    ) -> DbResult<Option<PhaseBAResult>> {
        self.write_domain.debug_check_permit(permit);
        let name_interned = index_def.name_interned;

        // ── Phase B: micro-barrier (raise → drain → lock) ──────────────────────
        // F-70 (#897): canonical drain-then-lock acquisition — raise
        // `REGULAR_INDEX_CREATE`, drain in-flight fast-path writers, THEN take
        // `unique_write_lock`. This order is load-bearing (see F-70 deadlock
        // proof in `writer_drain_barrier.rs` and `f70_lock_order_inversion_tests.rs`).
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                permit,
            )
            .await;

        // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
        // holding `ddl_admission` (via the barrier above) — see
        // `any_index_exists`'s doc and `create_index`'s matching check for
        // why the admission-guarded window (not a handler-layer check before
        // admission) is what closes the TOCTOU gap. `index_exists` (this
        // family's own occupancy) is enforced by `register_index_at_building`
        // below; this additionally rejects a name already used by ANY OTHER family.
        if self.any_index_exists(name).await {
            return Err(shamir_storage::error::DbError::KeyExists(format!(
                "index '{name}' already exists on this table (possibly in a different \
                 index family — names are unique per table across all families)"
            )));
        }

        // Read the allocation high water only AFTER raise → drain → lock.
        // It is an exact cut for this table: pre-barrier writers have drained,
        // slow-path writers are excluded by the lock, and later writes enter
        // the dirty-set before the barrier is released. Versions parked on
        // other tables can hold W back without hiding this table's rows.
        // If changefeed is absent, return the fallback signal.
        let Some(guard) = self.open_index_build_snapshot().await else {
            // Barrier guard drops here (RAII), releasing bit+lock.
            return Ok(None);
        };

        let pin = guard.version();

        // Capture paths before moving index_def.
        let paths = index_def.paths.clone();

        // ── Register at Building (same sequence as create_index_from_stream Phase 1) ──
        // F-72 (#899, P0): register first at Building, durably persist, then
        // backfill. This makes the index planner-invisible until Phase D flips
        // it to Ready, avoiding half-populated index queries.
        self.index_manager
            .register_index_at_building(index_def)
            .await?;

        // #1058: mark in-flight so live write-hooks capture to dirty-set.
        self.index_manager.mark_build_in_flight(name_interned);

        // P-1060: test seam - pause after mark_build_in_flight, before dropping barrier.
        // At this point: Building is durably persisted, in-flight flag is set,
        // barrier is still held.
        #[cfg(test)]
        {
            if let Some(hook) = self.phase_b_pause_hook.load_full() {
                hook.wait_at_window().await;
            }
        }

        // ── Drop barrier guards (RAII) ─────────────────────────────────────────
        // Phase A is barrier-free — writers must be able to proceed while we scan.
        // The SnapshotGuard (`guard`) stays alive through Phase A to pin the
        // version floor, but the barrier bit and lock are released now.
        drop(_barrier);
        drop(_uwl_guard);

        // ── Phase A: barrier-free backfill scan ─────────────────────────────────
        // `guard` (SnapshotGuard) remains alive through the scan — it pins the
        // version floor for the snapshot_stream. Writers can now proceed
        // concurrently; any write that touches the indexed paths is captured
        // in the dirty-set (activated by mark_build_in_flight above). Phase C
        // (#1088) drains and applies those deltas.

        let mvcc = self.mvcc_store().ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "phase_b_a_backfill: mvcc_store unavailable but open_index_build_snapshot succeeded"
                    .to_string(),
            )
        })?;

        // Stream from the pinned snapshot (same at_version for all batches).
        let stream = mvcc.snapshot_stream(batch_size, pin);

        // Adapt each batch: decode (Bytes, Bytes) to (RecordId, InnerValue).
        let posting_stream = stream.map(move |batch_result| {
            batch_result.and_then(|batch| {
                batch
                    .into_iter()
                    .map(|(key_bytes, value_bytes)| {
                        let record_id = RecordId::try_from_bytes(&key_bytes).ok_or_else(|| {
                            shamir_storage::error::DbError::Internal(
                                "Failed to parse RecordId from key".to_string(),
                            )
                        })?;

                        let inner_value = InnerValue::from_bytes(&value_bytes).map_err(|e| {
                            shamir_storage::error::DbError::Internal(format!(
                                "Failed to decode InnerValue: {e}"
                            ))
                        })?;

                        Ok((record_id, inner_value))
                    })
                    .collect::<DbResult<Vec<_>>>()
            })
        });

        // ── Backfill: batch-write postings (same pattern as create_index_from_stream Phase 2) ──
        let mut count = 0usize;

        let backfill_start = std::time::Instant::now();
        let mut last_progress_log = std::time::Instant::now();
        let mut batch_no = 0u64;
        const BACKFILL_PROGRESS_LOG_INTERVAL: std::time::Duration =
            std::time::Duration::from_secs(5);

        let mut posting_stream = Box::pin(posting_stream);

        while let Some(batch_result) = posting_stream.next().await {
            // Test seam: pause mid-scan after processing at least one batch.
            #[cfg(test)]
            {
                if batch_no == 1 {
                    if let Some(hook) = self.online_index_backfill_hook.load_full() {
                        hook.wait_at_window().await;
                    }
                }
            }

            let batch = batch_result.map_err(|e| {
                let detail = format!(
                    "CREATE INDEX '{name_interned}': online build backfill (Phase A) failed: {e}. \
                         The index is Building (planner-invisible) but under-populated."
                );
                log::error!("{}", detail);
                shamir_index::backend::contextualize_db_error(e, detail)
            })?;

            // Declare per-batch accumulators FRESH inside the loop (no shadowing).
            let mut posting_writes: Vec<(bytes::Bytes, bytes::Bytes)> = Vec::with_capacity(131_072);
            let mut cache_index_keys: Vec<bytes::Bytes> = Vec::with_capacity(131_072);

            for (record_id, value) in &batch {
                if let Some(irk) = build_index_key_from_record(false, name_interned, value, &paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    posting_writes.push((posting_key, bytes::Bytes::new()));
                    cache_index_keys.push(index_key);
                    count += 1;
                }
            }

            if !posting_writes.is_empty() {
                // Use the new IndexManager helper to write the batch and clear cache.
                self.index_manager
                    .write_postings_batch(posting_writes, cache_index_keys)
                    .await
                    .map_err(|e| {
                        let detail = format!(
                            "CREATE INDEX '{name_interned}': online build backfill (Phase A) \
                             failed: {e}. The index is Building (planner-invisible) but \
                             under-populated."
                        );
                        log::error!("{}", detail);
                        shamir_index::backend::contextualize_db_error(e, detail)
                    })?;
            }

            batch_no += 1;
            if last_progress_log.elapsed() >= BACKFILL_PROGRESS_LOG_INTERVAL {
                log::info!(
                    "CREATE INDEX '{}': online backfill (Phase A) in progress — {} rows \
                     indexed across {} batches ({:.1}s elapsed)",
                    name_interned,
                    count,
                    batch_no,
                    backfill_start.elapsed().as_secs_f64()
                );
                last_progress_log = std::time::Instant::now();
            }
        }

        // ── Return SnapshotGuard and pin to caller (Phase C/D) ─────────────────────
        // The guard must stay alive through Phase C/D to support pin-time reads.
        // Caller is responsible for dropping it after Phase D completes.
        log::info!(
            "CREATE INDEX '{}': Phase B+A completed — {} rows indexed in {:.1}s, index \
             left in Building state (dirty-set capture active)",
            name_interned,
            count,
            backfill_start.elapsed().as_secs_f64()
        );

        Ok(Some(PhaseBAResult { guard, pin }))
    }

    pub(crate) async fn phase_c_d_catchup_and_publish(
        &self,
        name_interned: u64,
        phase_ba: PhaseBAResult,
        permit: &RepoWritePermit,
    ) -> DbResult<()> {
        self.write_domain.debug_check_permit(permit);
        let PhaseBAResult { guard, pin } = phase_ba;

        // ── Phase C: barrier-free catch-up loop ─────────────────────────────
        for (iteration, _) in (0..Self::CATCHUP_ITERATION_CAP).enumerate() {
            // P-1060: test seam - pause at the top of the catch-up loop (first iteration).
            // At this point: Building is on disk, Phase A postings are durable,
            // SnapshotGuard is held, no barrier.
            #[cfg(test)]
            {
                if iteration == 0 {
                    if let Some(hook) = self.phase_c_pause_hook.load_full() {
                        hook.wait_at_window().await;
                    }
                }
            }

            let _ = iteration; // Used in cfg(test) block above

            let dirty = self.index_manager.drain_dirty_set(name_interned);
            if dirty.is_empty() {
                break;
            }
            self.apply_catchup_for_ids(name_interned, &dirty, pin)
                .await?;
        }

        // ── Phase D: short publish barrier ──────────────────────────────────
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier_admitted(
                crate::index::write_barrier_flags::REGULAR_INDEX_CREATE,
                permit,
            )
            .await;

        // P-1060: test seam - pause after barrier acquisition, before final residual drain.
        // At this point: Building is STILL on disk (the flip is the last step),
        // barrier is held.
        #[cfg(test)]
        {
            if let Some(hook) = self.phase_d_pause_hook.load_full() {
                hook.wait_at_window().await;
            }
        }

        // Final residual — whatever accumulated since the loop above's last
        // drain. Bounded by construction (the loop only exits on empty or cap).
        let final_dirty = self.index_manager.drain_dirty_set(name_interned);
        if !final_dirty.is_empty() {
            self.apply_catchup_for_ids(name_interned, &final_dirty, pin)
                .await?;
        }

        // Flip Building -> Ready + persist — mirror index_manager.rs:1645-1673
        // (create_index_from_stream's Phase 3) EXACTLY: flip in-memory first,
        // then save_index_info(), matching the existing publish-then-persist
        // ordering invariant (F-72/#899) documented there.
        self.index_manager
            .flip_to_ready(name_interned)
            .await
            .map_err(|e| {
                let detail = format!(
                    "CREATE INDEX '{name_interned}': catch-up completed and the \
                     index was flipped to Ready in memory, but the final durable \
                     persist of the Ready state (Phase D) failed: {e}. The index is \
                     queryable in THIS process but durably Building on disk — a \
                     restart will reload it as Building (planner-invisible). Call \
                     TableManager::verify() to confirm state, or \
                     TableManager::repair() to rebuild it."
                );
                log::error!("{}", detail);
                shamir_index::backend::contextualize_db_error(e, detail)
            })?;

        self.index_manager.clear_build_in_flight(name_interned);

        drop(guard); // release the pin — Phase C/D's last use of get_at(pin) was above.
                     // _barrier / _uwl_guard drop via RAII at function end.

        Ok(())
    }

    /// Shared by Phase C's loop and Phase D's final residual: batched
    /// pin-vs-current read for `ids`, then one `apply_catchup_batch` call.
    #[allow(dead_code)]
    async fn apply_catchup_for_ids(
        &self,
        name_interned: u64,
        ids: &[shamir_types::types::record_id::RecordId],
        pin: u64,
    ) -> DbResult<()> {
        let mvcc = self.mvcc_store().ok_or_else(|| {
            shamir_storage::error::DbError::Internal(
                "apply_catchup_for_ids: mvcc_store unavailable mid-catchup".to_string(),
            )
        })?;

        let keys: Vec<bytes::Bytes> = ids.iter().map(|id| id.to_bytes()).collect();
        let at_pin = mvcc.get_at_many(&keys, pin).await?;
        let at_now = self.get_many(ids).await?; // TableManager::get_many, already
                                                // decodes to InnerValue (table_manager_crud.rs:607)

        let mut deltas = Vec::with_capacity(ids.len());
        for i in 0..ids.len() {
            let old_value = at_pin[i]
                .as_ref()
                .map(InnerValue::from_bytes)
                .transpose()
                .map_err(|e| {
                    shamir_storage::error::DbError::Internal(format!(
                        "Phase C: failed to decode pin-time value: {e}"
                    ))
                })?;
            deltas.push((ids[i], old_value, at_now[i].clone()));
        }

        self.index_manager
            .apply_catchup_batch(name_interned, deltas)
            .await
    }
}
