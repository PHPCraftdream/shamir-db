use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;

use super::table_manager::TableManager;
use crate::index::sorted_index_manager::SortedIndexDefinition;

impl TableManager {
    /// Register a new sorted (B-tree-by-value) index over a single
    /// scalar field, then backfill it from existing records.
    ///
    /// cancel-safe: NO — `register` persists the definition, then
    /// the backfill streams existing rows into the new index.
    /// Cancellation after register but before/during the backfill
    /// loop leaves a registered sorted index with partial entries;
    /// the doctor's `repair()` rebuilds the index from scratch as a
    /// recovery path. Do NOT call under `tokio::select!` /
    /// `tokio::time::timeout`.
    ///
    /// # Concurrency (F-57, #883)
    ///
    /// Unlike the other three index families, sorted indexes MUST register
    /// BEFORE backfill: the backfill calls `on_record_created` through the
    /// `SortedIndexManager`, which needs the registered definition to know
    /// where to write. This makes the concurrent-writer exposure MORE acute
    /// (a writer can interleave with the backfill on a registered index), not
    /// less. F-57 closes the concurrent-writer race with the SAME barrier +
    /// lock + drain pattern as the other three families: `unique_write_lock`
    /// is held across the entire register→backfill sequence, the
    /// `SORTED_INDEX_CREATE` bit (F-69, #896: one bit of the single packed
    /// `write_barrier_flags` word) is raised so `needs_write_barrier()`
    /// returns `true`, and `drain_writers()` waits for any in-flight fast-path
    /// writer that read `false` before the flag went up. The cancellation
    /// residual (partial index on `select!`/`timeout`) remains — the doctor's
    /// `repair()` is the documented recovery path.
    pub async fn create_sorted_index(&self, index_name: &str, field_path: &[&str]) -> DbResult<()> {
        self.create_sorted_index_with_include(index_name, field_path, Vec::new())
            .await
    }

    /// Create a sorted index, optionally recording covering-index `included_fields`
    /// in the persisted metadata.
    pub async fn create_sorted_index_with_include(
        &self,
        index_name: &str,
        field_path: &[&str],
        included_fields: Vec<Vec<String>>,
    ) -> DbResult<()> {
        let interner = self.interner.get().await?;
        let name_interned = interner
            .touch_ind(index_name)
            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
            .key()
            .id();
        // #1003: mark THIS index's name as in-flight for the rest of the
        // method body — see `TableManager::create_index`'s matching guard +
        // `in_flight_create_guard`'s module doc. The sorted family registers
        // at `Building` BEFORE the backfill loop below, so without this
        // guard a healthy in-progress sorted-index create would
        // false-positive `degraded_index_count()` for the whole backfill
        // duration.
        let _in_flight = self.in_flight_creates.enter(name_interned);
        let mut path_ids: Vec<u64> = Vec::new();
        for seg in field_path {
            for part in seg.split('.') {
                let id = interner
                    .touch_ind(part)
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
                    .key()
                    .id();
                path_ids.push(id);
            }
        }
        // F-42 (#850): the covering-index `included_fields` segments must
        // be interned in-memory BEFORE the durable persist call lands, so
        // their ids are saved in the same chunk as the name + field-path
        // ids. Pre-F-42, `intern_included_paths` ran AFTER register — its
        // `touch_ind` side-effects landed AFTER the persist, so a persist
        // failure (or a crash between persist and `intern_included_paths`)
        // could leave the registered index's covering projection pointing
        // at un-persisted ids. Building `included_fields_interned` inline
        // here and constructing the def with `with_included_interned`
        // pre-populates the transient cache the covering projection reads,
        // so `intern_included_paths` is no longer needed at create time
        // (its other call site in `TableManager::create` still rebuilds
        // the cache for defs loaded from disk after restart).
        let mut included_fields_interned: Vec<Vec<u64>> = Vec::with_capacity(included_fields.len());
        for path_segs in &included_fields {
            let mut seg_ids: Vec<u64> = Vec::with_capacity(path_segs.len());
            for seg in path_segs {
                let id = interner
                    .touch_ind(seg.as_str())
                    .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?
                    .key()
                    .id();
                seg_ids.push(id);
            }
            included_fields_interned.push(seg_ids);
        }
        // F-70 (#897, P0): canonical drain-then-lock acquisition — raise
        // `SORTED_INDEX_CREATE`, drain in-flight fast-path writers, THEN take
        // `unique_write_lock`. F-57 (#883) originally acquired the lock
        // FIRST here, which this task found deadlocks against
        // `pre_commit_prelock`'s drain-guard-then-lock shape on a second
        // table. The register-before-backfill shape means a concurrent
        // writer could otherwise interleave with the backfill on a
        // registered-but-incomplete index — the barrier+drain closes that
        // regardless of lock-acquisition order (see
        // `TableManager::begin_write_barrier` and `writer_drain_barrier`'s
        // "F-70" doc section).
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::SORTED_INDEX_CREATE)
            .await;
        // R0-C (#1010): cross-family name-uniqueness preflight, done WHILE
        // holding `ddl_admission` (via the barrier above) — see
        // `TableManager::any_index_exists`'s doc and `create_index_v2`'s
        // matching check for why the admission-guarded window is what
        // closes the TOCTOU gap. `sorted_index_exists` (this family's own
        // occupancy) is enforced separately by `SortedIndexManager::register`;
        // this additionally rejects a name already used by ANY OTHER family.
        if self.any_index_exists(index_name).await {
            return Err(shamir_storage::error::DbError::KeyExists(format!(
                "index '{index_name}' already exists on this table (possibly in a \
                 different index family — names are unique per table across all families)"
            )));
        }
        // F-42 (#850) — same fix class as `create_index`/
        // `create_unique_index`: persist the interner's newly-touched
        // ids BEFORE `register` publishes the index. A persist failure
        // aborts before publish, so no rollback is needed (nothing was
        // registered yet). The subsequent `register` + backfill only need
        // the fully-built `def` (whose ids are already in-memory) and the
        // record stream — neither depends on the interner having been
        // durably persisted.
        self.interner.persist().await?;
        // F-72 (#899, P0): construct with `state = Building` — see
        // `SortedIndexDefinition::state`'s doc. `register` persists this
        // BEFORE the backfill loop runs (unchanged register-first ordering —
        // that's what closes the lost-write race against concurrent writers,
        // see the doc above), but a `Building` definition is invisible to
        // every PLANNER lookup (`find_by_field_ready`), so a concurrent
        // range/seek/order-by query cannot be routed to this half-populated
        // index while the backfill below is still streaming.
        let mut def = SortedIndexDefinition::with_included_interned(
            name_interned,
            path_ids,
            included_fields,
            included_fields_interned,
        );
        def.state = crate::index2::state::IndexState::Building;
        self.sorted_indexes.register(def).await?;

        // Backfill: stream existing records and add each to the new
        // sorted index. Avoids materialising the whole table.
        // P4 (pre-refactor boundary): read CURRENT state through the seam
        // (`self.list_stream` → MvccStore::current_stream when attached), not
        // `self.table.list_stream` directly, so collapse-main swaps one place.
        // F-57: the barrier + lock above ensure no concurrent writer can
        // interleave with this backfill — the snapshot is a true point-in-time
        // view with no in-flight writers.
        //
        // F-72 (#899, P0): backfill error/cancellation handling. If the loop
        // below returns `Err` (a `?` on a stream/decode/apply failure), the
        // definition stays registered but `Building` — it is NEVER flipped to
        // `Ready`, so it remains permanently planner-invisible (the state
        // flip only happens after this loop completes AND `mark_ready_at`
        // runs, below). Unlike index2, the base_index sorted-index family has NO
        // automatic restart-from-scratch self-heal at table-open time (grep-
        // verified: `TableManager::create`'s open path re-hydrates
        // `SortedIndexManager` via `load()`, which restores whatever `state`
        // was last persisted, but runs no backfill-repair loop for a
        // `Building` entry the way the index2 branch does). A crash or error
        // here therefore leaves a `Building`, planner-invisible, partially-
        // populated definition that self-heals ONLY via an explicit operator
        // `doctor::repair()` call (which rebuilds every definition
        // unconditionally, regardless of state) — it does NOT silently
        // resurrect as queryable on its own. This is an accepted, explicitly
        // documented gap (mirrors the pre-existing `create_sorted_index`
        // doc's "cancel-safe: NO" note) — automatic base_index-family
        // self-healing is out of scope for this task.
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        // P1-4 (#969): periodic progress log so an operator watching logs can
        // see the DDL is progressing, not hung, during a long backfill scan.
        let backfill_start = std::time::Instant::now();
        let mut last_progress_log = std::time::Instant::now();
        let mut sorted_count = 0usize;
        let mut sorted_batch_no = 0u64;
        // P1-2 (#967): the `register` call above already durably persisted the
        // Building definition. Enrich any backfill failure with the partial-
        // state context so the caller knows the index is stuck as Building.
        let enrich_backfill = |e: shamir_storage::error::DbError| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE SORTED INDEX '{index_name}': the index definition was \
                 durably registered as Building, but the backfill failed: {e}. \
                 The index is NOT queryable — it remains permanently Building \
                 (planner-invisible) until rebuilt. Call TableManager::verify() \
                 to confirm state, or TableManager::repair() to rebuild it."
            ))
        };
        while let Some(batch) = stream.next().await {
            let batch = match batch {
                Ok(b) => b,
                Err(e) => return Err(enrich_backfill(e)),
            };
            for (id, cow) in batch {
                let record = match cow.into_inner() {
                    Ok(r) => r,
                    Err(e) => return Err(enrich_backfill(e)),
                };
                // F-71 (#898): the backfill is NOT a real MVCC write — it has
                // no commit version of its own, so every `on_record_created`
                // call here still passes the literal `0` placeholder used
                // since this backfill loop was written. That placeholder is
                // ONLY safe because `mark_ready_at` below overwrites the
                // resulting epoch with the table's real watermark before
                // this method returns; see that call's doc for why leaving
                // the epoch at `0` (the bug this task fixes) would let an
                // `AsOf` query pinned to any version BEFORE this create
                // wrongly take the fast path against an index that in fact
                // mirrors state as of `table_version`.
                self.sorted_indexes
                    .on_record_created(&id, &record, 0)
                    .await
                    .map_err(enrich_backfill)?;
                sorted_count += 1;
            }
            sorted_batch_no += 1;
            if last_progress_log.elapsed() >= std::time::Duration::from_secs(5) {
                log::info!(
                    "CREATE SORTED INDEX '{}': backfill in progress — {} rows \
                     indexed across {} batches ({:.1}s elapsed)",
                    index_name,
                    sorted_count,
                    sorted_batch_no,
                    backfill_start.elapsed().as_secs_f64()
                );
                last_progress_log = std::time::Instant::now();
            }
            // F-72 (#899, P0) test seam: park here (mid-backfill, definition
            // still `Building` and hence planner-invisible) if a test
            // installed a pause hook. Zero cost in the real path (`None`),
            // compiled out of non-test builds. Lets a regression test drive
            // a concurrent READ into the exact window this task closes and
            // assert it falls back to a full scan instead of observing a
            // half-populated index. See `create_index2_backfill_hook`'s
            // sibling doc for the analogous index2 seam.
            #[cfg(test)]
            if let Some(hook) = self.create_sorted_index_backfill_hook.load_full() {
                hook.wait_at_window().await;
            }
        }
        // F-71 (#898): close vector 2 of the F-67 regression — mark the
        // index READY as of the table's CURRENT committed watermark, sampled
        // now that the backfill stream (still under the write barrier + lock
        // acquired above, so no writer could have landed a commit invisible
        // to the snapshot the backfill just read) has fully drained. Without
        // this, the index's epoch stays at whatever the `on_record_created(
        // .., 0)` calls above left it — `0` for a brand-new index — even
        // though its postings mirror everything up to `table_version`; an
        // `AsOf` query pinned to any version strictly before this CREATE
        // would then wrongly see `0 <= pinned` and take the seek fast path
        // against an index that does not actually reflect the pinned
        // snapshot (e.g. a row deleted between the pin and this backfill
        // would be silently omitted rather than correctly included).
        //
        // Read the watermark off the attached `MvccStore` directly
        // (`MvccStore::current_committed_version`), NOT `self.changefeed`.
        // `changefeed` is a SEPARATE, narrower wire (only present when SSI
        // footprint recording was explicitly attached via `with_changefeed`)
        // — plenty of legitimate MVCC-backed tables have `mvcc_store: Some`
        // but `changefeed: None` (every test harness that skips
        // `with_changefeed`, e.g. `f53b_asof_seek_tests.rs`'s
        // `make_mvcc_score_table`, and any production table wired without
        // SSI). Gating on `changefeed` there would silently floor the epoch
        // at `0` — reproducing exactly the bug this task fixes — for every
        // such table. `current_committed_version()` reads `0` only when NO
        // `MvccStore` is attached at all (system tables / pure in-memory
        // tests without MVCC) — `mark_ready_at` still floors the epoch there,
        // matching the pre-existing "epoch 0 for an index this process has
        // not observed a mutation for" semantics: those tables have no
        // MvccStore for `read_as_of` to even run against, so the gate is moot
        // for them.
        let table_version = self
            .mvcc_store_ref()
            .map(|mvcc| mvcc.current_committed_version())
            .unwrap_or(0);
        self.sorted_indexes
            .mark_ready_at(name_interned, table_version)
            .await
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "CREATE SORTED INDEX '{index_name}': the backfill completed, \
                     but the final Ready flip + persist (mark_ready_at) failed: \
                     {e}. The index is queryable in THIS process but durably \
                     Building on disk — a restart will reload it as Building \
                     (planner-invisible). Call TableManager::verify() to confirm \
                     state, or TableManager::repair() to rebuild it."
                ))
            })?;
        log::info!(
            "Created sorted index '{}' with {} entries in {:.1}s",
            index_name,
            sorted_count,
            backfill_start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// Drop a sorted index by name.
    ///
    /// R0-A (#1012): wrapped in `begin_write_barrier(SORTED_INDEX_CREATE)` —
    /// same drain-then-lock pattern as `create_sorted_index_with_include`,
    /// reusing the family's existing CREATE bit rather than minting a
    /// dedicated DROP bit (no concurrent DROP-vs-CREATE overlap is
    /// tolerable for this family — see `create_sorted_index_with_include`'s
    /// F-57 doc for why a writer or a second DDL op racing the definition
    /// swap is unsafe). Before this fix, `drop_index`'s own doc claimed
    /// "TOCTOU-safe under the engine's write barrier (drop_sorted_index is
    /// serialized via begin_write_barrier)" — a stale assertion, since this
    /// call site never actually acquired one; this closes that gap so the
    /// claim is true. Held across the ENTIRE tombstone → definition-retire →
    /// posting-sweep → persist → tombstone-clear sequence inside
    /// `SortedIndexManager::drop_index`.
    ///
    /// #1067: accepts `op_id` (mirrors `drop_index`/`drop_unique_index`'s
    /// convention), threaded down to `SortedIndexManager::drop_index` as
    /// `op_id.map(|id| id.to_string())` so the terminal `DdlOpStatus` write
    /// carries the caller's correlation id.
    pub async fn drop_sorted_index(
        &self,
        index_name: &str,
        op_id: Option<RecordId>,
    ) -> DbResult<bool> {
        let interner = self.interner.get().await?;
        let Some(name_interned) = interner.get_ind(index_name) else {
            return Ok(false);
        };
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::SORTED_INDEX_CREATE)
            .await;
        self.sorted_indexes
            .drop_index(
                name_interned.id(),
                op_id.map(|id| id.to_string()),
                Some(index_name),
            )
            .await
    }
}
