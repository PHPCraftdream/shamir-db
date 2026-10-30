use std::time::Instant;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::RecordKey;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::types::record_id::RecordId;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_keys::{build_index_key_from_record, build_posting_key};
use crate::base_index::write_barrier_flags::REGULAR_INDEX_EXISTS;
use shamir_types::types::value::InnerValue;

use super::inner::{
    fire_post_flag_set_pre_gen_bump_test_hook, IndexManager, BACKFILL_PROGRESS_LOG_INTERVAL,
};

impl IndexManager {
    /// Создаёт новый индекс для таблицы.
    ///
    /// Процесс создания:
    /// 1. Регистрирует определение индекса (чтобы live write-hook
    ///    начал поддерживать постинги для конкурентных записей).
    /// 2. Потоково читает `data_store` батчами по `FULL_SCAN_BATCH`
    ///    записей и для каждого батча строит постинги и флашит их
    ///    через `set_many` — память ограничена O(батч), а не O(таблица).
    ///
    /// # Аргументы
    ///
    /// * `index_def` — определение индекса (имя, пути полей, уникальность)
    ///
    /// # Производительность
    ///
    /// Потоковая обработка: каждый батч (`FULL_SCAN_BATCH` записей)
    /// обрабатывается независимо — декодируется, индексируется и
    /// флашится в `info_store` через `set_many`. Пиковая память
    /// ограничена размером батча, а не размером всей таблицы.
    pub async fn create_index(&self, index_def: IndexDefinition) -> DbResult<()> {
        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE INDEX for a name whose DROP
        // is still in flight — a tombstoned name has had (or is having) its
        // postings swept; a fresh create would inherit orphan ghost postings
        // keyed by the same `name_interned`.
        if self
            .dropping_regular
            .lock()
            .unwrap()
            .contains_key(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create regular index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST ──────────────────────────
        // (Same concurrency rationale as `create_index_from_records`.)
        self.indexes.add_index(index_def);
        // #1102: set REGULAR_INDEX_EXISTS BEFORE bumping generation (mirrors
        // #1098's fix for UNIQUE_INDEX_EXISTS). See `create_index_from_records`
        // for the full ordering argument.
        self.write_barrier_flags.set(REGULAR_INDEX_EXISTS);
        // #1102 test-only pause seam: fires strictly AFTER the flag set above
        // and BEFORE the generation bump below. No-op in every non-test build.
        fire_post_flag_set_pre_gen_bump_test_hook(&self.post_flag_set_pre_gen_bump_hook).await;
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.save_index_info().await?;

        // ── Phase 2: incremental backfill, batch-by-batch ───────────────────
        // Each batch is decoded, indexed, and flushed independently. Memory
        // is bounded by the batch size (O(batch)), not the table size.
        // Idempotent: a record also written live by the hook right at the
        // registration boundary yields the same (key, empty-value) pair.
        //
        // NOTE: we decode each record into a full `InnerValue` (same as the
        // materialized path in `create_index_from_records`) to guarantee
        // byte-identical posting keys. The zero-copy `RecordView` lens
        // produces DIFFERENT scalar hashes for some value types (e.g. f64
        // encoding edge cases in the msgpack wire form), so it cannot be
        // used here without breaking index-identity. A follow-up should
        // reconcile the lens's scalar decoding with `InnerValue`'s and
        // then switch this path to `RecordView` for zero-copy indexing.
        //
        // F-2 (#1028): a malformed key (not exactly 16 bytes) or an
        // undecodable value now ABORTS the backfill with a typed
        // `DbError::Codec`, instead of silently `continue`-ing past the
        // row. Fail-open here previously left the row un-indexed while the
        // index was still marked `Ready` — a later query planned through
        // this index would silently never return that row, even though a
        // full scan would find it (worse than the unique-index case #1023
        // already fixed: that one risks an unconstrained duplicate; this
        // one silently drops rows from query results). Mirrors #1023's
        // fail-closed fix for `create_unique_index`'s backfill
        // (`index_manager_unique.rs`).
        let mut count = 0usize;
        let mut stream = self.data_store.iter_stream(FULL_SCAN_BATCH);
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let mut posting_writes: Vec<(Bytes, Bytes)> =
                Vec::with_capacity(batch.len().min(131_072));
            let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(posting_writes.capacity());
            for (key_bytes, value_bytes) in &batch {
                let arr: [u8; 16] = key_bytes.as_ref().try_into().map_err(|_| {
                    shamir_storage::error::DbError::Codec(format!(
                        "create_index backfill: malformed record key, expected 16-byte \
                         RecordId, got {} bytes (genuine corruption — fail-closed, aborting \
                         backfill rather than silently skipping the row)",
                        key_bytes.as_ref().len()
                    ))
                })?;
                let record_id = RecordId(arr);
                let value = InnerValue::from_bytes(value_bytes).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "create_index backfill: record {record_id:?} value failed to decode: \
                         {e} (fail-closed, aborting backfill rather than silently skipping \
                         the row)"
                    ))
                })?;
                if let Some(irk) = build_index_key_from_record(false, name_interned, &value, &paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, &record_id);
                    posting_writes.push((posting_key, Bytes::new()));
                    cache_index_keys.push(index_key);
                    count += 1;
                }
            }
            if !posting_writes.is_empty() {
                // Boundary: posting keys are built as `Bytes`; the store
                // `set_many` now takes `RecordKey` (byte-identical conversion).
                let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v))
                    .collect();
                self.info_store.set_many(posting_writes).await?;
            }
            for ik in cache_index_keys {
                self.posting_cache_generation
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                self.posting_cache.remove(&ik);
            }
        }

        log::info!(
            "Created index '{}' with {} entries (streamed)",
            name_interned,
            count
        );
        Ok(())
    }

    /// FINAL-A: create index and backfill from an already-decoded record
    /// stream instead of `data_store.iter_stream`. Used by `TableManager`
    /// when an MvccStore is attached — the seam (`list_stream`) is the
    /// sole source of truth after FINAL-A.
    ///
    /// # Concurrency (audit A9 — register-first, backfill-second)
    ///
    /// The index definition is registered (`add_index`) BEFORE backfilling
    /// postings. This closes the lost-write window: any concurrent writer
    /// that lands AFTER the snapshot was taken but BEFORE registration is
    /// now caught by the live write-hook (`on_record_created`), which sees
    /// the freshly-registered definition and maintains a posting for it.
    ///
    /// A record captured by BOTH the snapshot AND a concurrent live write
    /// (a narrow overlap right at the registration boundary) produces the
    /// SAME physical posting key — `build_posting_key(index_key, record_id)`
    /// is a pure function of `(is_unique, name_interned, h1, h2, record_id)`
    /// with an EMPTY value (`Bytes::new()`) — so writing it twice is an
    /// idempotent no-op, not a corruption.
    ///
    /// # Planner invisibility + persist ordering (F-72, #899, P0)
    ///
    /// `index_def` is registered at `state = Building` (set by the caller,
    /// `TableManager::create_index` — see that call site) — register-first
    /// is UNCHANGED (it is what closes the lost-write race above); what
    /// changes is that a `Building` definition is invisible to every
    /// PLANNER lookup (`TableManager::find_single_field_index`,
    /// `try_plan_and_index_scan` — both now consult
    /// `IndexManager::iter_indexes_ready`, not the raw `iter_indexes`), so a
    /// concurrent Eq/In/And query cannot be routed to this half-populated
    /// index while Phase 2 below is still streaming postings.
    ///
    /// The FIRST `save_index_info` (right after `add_index`) durably
    /// publishes the `Building` marker BEFORE the backfill starts — so a
    /// crash mid-backfill leaves a durable, planner-invisible `Building`
    /// definition on disk, not a `Ready` one with missing postings. Once the
    /// backfill (Phase 2) completes, the definition is flipped to `Ready`
    /// in-memory and a SECOND `save_index_info` durably persists that flip —
    /// this fixes the pre-F-72 publish-then-persist inversion (the state
    /// flip to `Ready`, i.e. "queryable", now happens no earlier than the
    /// persist that makes it durable, never after — a persist failure at
    /// the SECOND save cannot leave a `Ready`, queryable, durably-unsaved-as-
    /// Ready index behind: `?` propagates the error and the in-memory
    /// `indexes` map still holds the flipped-to-Ready struct, but the
    /// CALLER receives `Err`, so the DDL statement is reported as failed —
    /// see `create_index`'s caller contract). On a genuine backfill error
    /// (Phase 2's `?` — none of today's in-loop steps are fallible in this
    /// specific method, but `plan`/`apply` sibling call sites are — or a
    /// future fallible step), the definition simply never reaches the
    /// second `save_index_info`/flip and stays durably `Building`.
    ///
    /// Unlike index2, the base_index `IndexManager` family has NO automatic
    /// restart-from-scratch self-heal for a `Building` definition at
    /// table-open time (grep-verified: `IndexManager::new` only loads
    /// definitions via `IndexInfo::decode_bytes`, it does not re-run any
    /// backfill). A `Building` definition left behind by a crash/error
    /// therefore stays durably `Building` — permanently planner-invisible,
    /// never silently resurrected as `Ready` — until an operator runs
    /// `TableManager::repair()` (`doctor.rs`), which rebuilds every
    /// definition unconditionally regardless of state. This is an accepted,
    /// explicitly-documented gap: automatic base_index-family self-healing is
    /// out of scope for this task (mirrors the equivalent note on
    /// `create_sorted_index_with_include`).
    pub async fn create_index_from_records(
        &self,
        index_def: IndexDefinition,
        records: Vec<(RecordId, InnerValue)>,
    ) -> DbResult<()> {
        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE for a name whose DROP is
        // still in flight (see `create_index`'s matching guard).
        if self
            .dropping_regular
            .lock()
            .unwrap()
            .contains_key(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create regular index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST, at Building ────────────
        // Once registered, the live write-hook starts maintaining postings
        // for every concurrent/new write against this index immediately —
        // but the definition stays planner-invisible until Phase 3 flips it
        // to Ready. `save_index_info` durably persists the `Building` marker
        // so a crash between registration and backfill leaves a correctly-
        // registered (but possibly under-populated), still-planner-invisible
        // index that the doctor `repair()` pass can top up on request.
        self.indexes.add_index(index_def);
        // #1102: set REGULAR_INDEX_EXISTS BEFORE bumping generation (mirrors
        // #1098's fix for UNIQUE_INDEX_EXISTS). The `set` is `SeqCst` (via
        // `write_barrier_flags.rs`), same as bit 0; the generation bump's
        // `fetch_add` is `AcqRel`. A reader that observes the NEW generation
        // via an `Acquire` load is guaranteed — by that synchronizes-with edge
        // plus program order on the single writer thread — to also observe
        // this flag set.
        self.write_barrier_flags.set(REGULAR_INDEX_EXISTS);
        // #1102 test-only pause seam: fires strictly AFTER the flag set above
        // and BEFORE the generation bump below — see
        // `fire_post_flag_set_pre_gen_bump_test_hook`'s doc. No-op in every
        // non-test build.
        fire_post_flag_set_pre_gen_bump_test_hook(&self.post_flag_set_pre_gen_bump_hook).await;
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.save_index_info().await?;

        // ── Phase 2: backfill postings from the snapshot ────────────────────
        // Idempotent: a record also written live by the hook right at the
        // registration boundary yields the same (key, empty-value) pair.
        let mut count = 0usize;
        let mut posting_writes: Vec<(Bytes, Bytes)> = Vec::with_capacity(131072);
        let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(131072);
        for (record_id, value) in &records {
            if let Some(irk) = build_index_key_from_record(false, name_interned, value, &paths) {
                let index_key = irk.to_bytes();
                let posting_key = build_posting_key(&index_key, record_id);
                posting_writes.push((posting_key, Bytes::new()));
                cache_index_keys.push(index_key);
                count += 1;
            }
        }
        if !posting_writes.is_empty() {
            // Boundary: posting keys are built as `Bytes`; the store
            // `set_many` now takes `RecordKey` (byte-identical conversion).
            let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                .into_iter()
                .map(|(k, v)| (k.into(), v))
                .collect();
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            // P1-2 (#967): if this backfill write fails AFTER Phase 1's
            // `save_index_info` already persisted the Building definition,
            // the index is durably registered but NOT queryable. Enrich
            // the error so the caller knows what happened and how to resolve.
            self.info_store
                .set_many(posting_writes)
                .await
                .map_err(|e| {
                    let detail = format!(
                        "CREATE INDEX '{name_interned}': the index definition was \
                     durably registered as Building (Phase 1 persist succeeded), \
                     but the backfill posting write (Phase 2) failed: {e}. The \
                     index is NOT queryable — it remains permanently Building \
                     (planner-invisible) until rebuilt. Call TableManager::verify() \
                     to confirm state, or TableManager::repair() to rebuild it."
                    );
                    log::error!("{}", detail);
                    crate::backend::contextualize_db_error(e, detail)
                })?;
        }
        for ik in cache_index_keys {
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.posting_cache.remove(&ik);
        }

        // F-72 (#899, P0) test seam: park here (postings written, definition
        // still `Building` and hence planner-invisible until Phase 3 below)
        // if a test installed a pause hook. `None` on every real path — one
        // uncontended `ArcSwapOption::load_full()` Acquire load, no lock, no
        // allocation. NOT `#[cfg(test)]`-gated — see the field's doc for why
        // (a cross-crate test consumer needs this reachable in a normal,
        // non-test compile of this crate). Lets a regression test drive a
        // concurrent READ into the exact window this task closes.
        if let Some(hook) = self.create_index_backfill_hook.load_full() {
            hook.wait_at_window().await;
        }

        // ── Phase 3: flip Building → Ready, persist BEFORE returning ────────
        // F-72 (#899, P0): the state flip and its durable persist are the
        // ONLY point a concurrent planner read may start observing this
        // index. Fixes the publish-then-persist inversion: the in-memory
        // flip happens here, immediately followed by the persist that makes
        // it durable — never the other way around (persist-after-serving is
        // what the OLD single-save-before-backfill shape effectively did
        // for `Ready`-by-default definitions). A failure on THIS save
        // surfaces as `Err` to the DDL caller (the statement is reported
        // failed) while the definition remains `Ready` in THIS process's
        // memory but durably `Building` on disk — an intentional choice
        // (see the method doc): we do not roll back the in-memory flip,
        // because the postings themselves are already fully written and
        // correct; a restart would simply re-observe `Building` from disk
        // and require an operator `repair()` to reconcile, at worst costing
        // a redundant full rebuild, never a correctness gap (the disk state
        // never claims Ready without the postings backing it, and the
        // planner in THIS still-live process is the only place `Ready` is
        // visible without yet being durable — acceptable because the
        // postings backing that Ready view already exist).
        // P1-2 (#967): if this Phase 3 persist fails, the index is Ready
        // in THIS process's memory but durably Building on disk — enrich
        // the error so the caller knows the state split and how to resolve.
        self.flip_to_ready(name_interned).await.map_err(|e| {
            let detail = format!(
                "CREATE INDEX '{name_interned}': the backfill completed and the \
                 index was flipped to Ready in memory, but the final durable \
                 persist of the Ready state (Phase 3) failed: {e}. The index is \
                 queryable in THIS process but durably Building on disk — a \
                 restart will reload it as Building (planner-invisible). Call \
                 TableManager::verify() to confirm state, or \
                 TableManager::repair() to rebuild it."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
        })?;

        log::info!(
            "Created index '{}' with {} entries (from seam)",
            name_interned,
            count
        );
        Ok(())
    }

    /// F-78 (#905): streaming counterpart of [`create_index_from_records`].
    ///
    /// Same externally-observable result and the SAME F-72 Phase 1 → Phase 2
    /// → Phase 3 lifecycle as [`create_index_from_records`]; only Phase 2's
    /// *body* differs — it consumes a `Stream` of decoded record *batches*
    /// instead of one pre-materialised `Vec`, so peak memory is O(batch) not
    /// O(table). Each batch is decoded, indexed, and flushed to `info_store`
    /// via its own `set_many` (one transactional commit per batch on
    /// transactional backends), then dropped before the next batch is read.
    ///
    /// # Why a separate method (not a signature change on the old one)
    ///
    /// [`create_index_from_records`] is preserved byte-for-byte unchanged so
    /// the F-78 correctness-equivalence test can build the SAME index the OLD
    /// (materialise-then-one-`set_many`) way and the NEW (stream-and-batch)
    /// way against identical fixtures and assert the resulting posting SETS
    /// are identical. The streaming rewrite must change HOW the postings are
    /// written, never WHAT gets written.
    ///
    /// # F-72 state machine — UNCHANGED
    ///
    /// Phase 1 (register at `Building`, durable persist) and Phase 3 (flip to
    /// `Ready`, durable persist) are identical to [`create_index_from_records`]
    /// — only Phase 2's materialise-then-iterate body is replaced by a
    /// stream-and-batch body. The `create_index_backfill_hook` pause point
    /// stays exactly where it was (post-Phase-2, pre-Phase-3), so F-72's
    /// regression tests are unaffected.
    ///
    /// # Concurrency — write-delta catch-up is FREE (reconfirmed)
    ///
    /// The caller (`TableManager::create_index`) holds F-70's write barrier
    /// (`begin_write_barrier(REGULAR_INDEX_CREATE)` → raise bit → drain →
    /// `unique_write_lock`) across the ENTIRE Phase 1→2→3 sequence, so no
    /// concurrent writer can land a row *during* this loop — writers observe
    /// `needs_write_barrier() == true` and queue on `unique_write_lock` until
    /// the barrier drops. A row written at the registration boundary (after
    /// the snapshot but before Phase 1's `add_index`) is caught by the LIVE
    /// write-hook that Phase 1 activates — the SAME register-first ordering
    /// `create_index_from_records` already relied on, which this method does
    /// NOT change. So streaming introduces no new lost-write window and needs
    /// no new catch-up mechanism: the existing register-first + live-hook +
    /// barrier ordering already covers it. (The barrier holds writers for the
    /// whole build in BOTH the old and new shapes, so streaming does not by
    /// itself reduce writer queueing — its benefit is peak *memory*, tracked
    /// by the F-78 bench. Reducing writer-blocked time would require releasing
    /// the barrier between batches, which is explicitly out of scope — see the
    /// brief's "do not disturb F-72 / only Phase 2's body is in scope" rule.)
    ///
    /// # Cancel-safety / partial-build residual (unchanged class)
    ///
    /// An `Err` from the stream or a `set_many` (`?` below) leaves the
    /// definition registered but `Building` — it never reaches Phase 3's
    /// flip, so it stays permanently planner-invisible until an operator
    /// `repair()` — the SAME accepted residual as
    /// [`create_index_from_records`] and `create_sorted_index_with_include`.
    pub async fn create_index_from_stream<S>(
        &self,
        index_def: IndexDefinition,
        stream: S,
    ) -> DbResult<()>
    where
        S: futures::Stream<Item = DbResult<Vec<(RecordId, InnerValue)>>> + Unpin,
    {
        let name_interned = index_def.name_interned;

        // P0-3 (#959) sub-bug 3b: reject CREATE for a name whose DROP is
        // still in flight (see `create_index`'s matching guard).
        if self
            .dropping_regular
            .lock()
            .unwrap()
            .contains_key(&name_interned)
        {
            return Err(shamir_storage::error::DbError::Internal(format!(
                "Cannot create regular index '{name_interned}': \
                 a DROP INDEX for this name is still in progress"
            )));
        }

        // Capture paths before `index_def` is moved into `add_index`.
        let paths = index_def.paths.clone();

        // ── Phase 1: register the definition FIRST, at Building ────────────
        // (Identical to `create_index_from_records` — see that method's doc.)
        self.indexes.add_index(index_def);
        // #1102: set REGULAR_INDEX_EXISTS BEFORE bumping generation (mirrors
        // #1098's fix for UNIQUE_INDEX_EXISTS). See `create_index_from_records`
        // for the full ordering argument.
        self.write_barrier_flags.set(REGULAR_INDEX_EXISTS);
        // #1102 test-only pause seam: fires strictly AFTER the flag set above
        // and BEFORE the generation bump below. No-op in every non-test build.
        fire_post_flag_set_pre_gen_bump_test_hook(&self.post_flag_set_pre_gen_bump_hook).await;
        self.bump_generation(); // P0-2 (#958): gen gate for commit-time rederive
        self.save_index_info().await?;

        // ── Phase 2: stream + batch-write postings (F-78 body) ──────────────
        // O(batch) peak memory: each batch's posting_writes / cache_index_keys
        // are built, flushed via `set_many`, and dropped before the next batch.
        // Idempotent against a record also written by the live hook at the
        // registration boundary — same (posting_key, empty-value) pair.
        let mut count = 0usize;
        let mut stream = stream;
        // P1-4 (#969): periodic progress log so an operator watching logs can
        // see the DDL is progressing, not hung, during a long backfill scan.
        // Time-gated (not batch-gated) so the cadence is consistent regardless
        // of the caller's batch size.
        let backfill_start = Instant::now();
        let mut last_progress_log = Instant::now();
        let mut batch_no = 0u64;
        // P1-2 (#967): enricher for any Phase 2 backfill failure — the
        // Building definition is already durably persisted by Phase 1 above.
        let enrich_backfill = |e: shamir_storage::error::DbError| {
            shamir_storage::error::DbError::Internal(format!(
                "CREATE INDEX '{name_interned}': the index definition was \
                 durably registered as Building (Phase 1 persist succeeded), \
                 but the streaming backfill (Phase 2) failed: {e}. The index \
                 is NOT queryable — it remains permanently Building \
                 (planner-invisible) until rebuilt. Call TableManager::verify() \
                 to confirm state, or TableManager::repair() to rebuild it."
            ))
        };
        while let Some(batch_result) = stream.next().await {
            let batch = match batch_result {
                Ok(b) => b,
                Err(e) => return Err(enrich_backfill(e)),
            };
            let mut posting_writes: Vec<(Bytes, Bytes)> =
                Vec::with_capacity(batch.len().min(131_072));
            let mut cache_index_keys: Vec<Bytes> = Vec::with_capacity(posting_writes.capacity());
            for (record_id, value) in &batch {
                if let Some(irk) = build_index_key_from_record(false, name_interned, value, &paths)
                {
                    let index_key = irk.to_bytes();
                    let posting_key = build_posting_key(&index_key, record_id);
                    posting_writes.push((posting_key, Bytes::new()));
                    cache_index_keys.push(index_key);
                    count += 1;
                }
            }
            if !posting_writes.is_empty() {
                // Boundary: posting keys are built as `Bytes`; the store
                // `set_many` takes `RecordKey` (byte-identical conversion).
                let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v))
                    .collect();
                self.posting_cache_generation
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                self.info_store
                    .set_many(posting_writes)
                    .await
                    .map_err(enrich_backfill)?;
            }
            for ik in cache_index_keys {
                self.posting_cache_generation
                    .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                self.posting_cache.remove(&ik);
            }
            batch_no += 1;
            if last_progress_log.elapsed() >= BACKFILL_PROGRESS_LOG_INTERVAL {
                log::info!(
                    "CREATE INDEX '{}': backfill in progress — {} rows indexed \
                     across {} batches ({:.1}s elapsed)",
                    name_interned,
                    count,
                    batch_no,
                    backfill_start.elapsed().as_secs_f64()
                );
                last_progress_log = Instant::now();
            }
        }

        // F-72 (#899, P0) test seam — IDENTICAL placement to
        // `create_index_from_records` (post-Phase-2, pre-Phase-3). NOT
        // `#[cfg(test)]`-gated — cross-crate test consumer; see the field's
        // doc. `None` on every real path.
        if let Some(hook) = self.create_index_backfill_hook.load_full() {
            hook.wait_at_window().await;
        }

        // ── Phase 3: flip Building → Ready, persist BEFORE returning ────────
        // (Identical to `create_index_from_records` — see that method's doc.)
        // P1-2 (#967): Phase 3 persist — same enrichment as
        // `create_index_from_records` (see that method's matching comment).
        self.flip_to_ready(name_interned).await.map_err(|e| {
            let detail = format!(
                "CREATE INDEX '{name_interned}': the backfill completed and the \
                 index was flipped to Ready in memory, but the final durable \
                 persist of the Ready state (Phase 3) failed: {e}. The index is \
                 queryable in THIS process but durably Building on disk — a \
                 restart will reload it as Building (planner-invisible). Call \
                 TableManager::verify() to confirm state, or \
                 TableManager::repair() to rebuild it."
            );
            log::error!("{}", detail);
            crate::backend::contextualize_db_error(e, detail)
        })?;

        log::info!(
            "Created index '{}' with {} entries in {:.1}s (streamed from seam)",
            name_interned,
            count,
            backfill_start.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// #1088: Flip an index from Building to Ready and persist the change.
    ///
    /// This performs the same flip sequence as Phase 3 of
    /// `create_index_from_records` and `create_index_from_stream`:
    /// - Flips the definition in-memory from Building to Ready
    /// - Persists the metadata to `info_store`
    ///
    /// This method is used by Phase D of the online build path.
    pub async fn flip_to_ready(&self, name_interned: u64) -> DbResult<()> {
        // Flip in-memory first, then persist (F-72 ordering).
        if let Some(def) = self.indexes.get_index(name_interned) {
            let mut ready_def = def;
            ready_def.state = crate::state::IndexState::Ready;
            self.indexes.add_index(ready_def);
        }
        self.save_index_info().await
    }

    /// #1087: Register an index at `Building` state (Phase 1 of online CREATE INDEX).
    ///
    /// This performs the same registration sequence as Phase 1 of
    /// `create_index_from_records` and `create_index_from_stream`:
    /// - Adds the definition to the in-memory index registry at `Building` state
    /// - Bumps the generation
    /// - Sets the `has_indexes` flag
    /// - Persists the metadata to `info_store`
    ///
    /// This method is used by the Phase B+A online build path, which performs
    /// registration here and then does a barrier-free backfill via Phase A.
    pub async fn register_index_at_building(&self, index_def: IndexDefinition) -> DbResult<()> {
        // Phase 1: register the definition FIRST, at Building
        // (Identical sequence to create_index_from_records Phase 1)
        self.indexes.add_index(index_def);
        // #1102: set REGULAR_INDEX_EXISTS BEFORE bumping generation (mirrors
        // #1098's fix for UNIQUE_INDEX_EXISTS). See `create_index_from_records`
        // for the full ordering argument.
        self.write_barrier_flags.set(REGULAR_INDEX_EXISTS);
        // #1102 test-only pause seam: fires strictly AFTER the flag set above
        // and BEFORE the generation bump below. No-op in every non-test build.
        fire_post_flag_set_pre_gen_bump_test_hook(&self.post_flag_set_pre_gen_bump_hook).await;
        self.bump_generation();
        self.save_index_info().await?;
        Ok(())
    }

    /// #1088: apply a batch of pin-vs-current posting diffs directly for a
    /// specific Building+in-flight def, bypassing the in-flight dirty-set-
    /// capture gate (`is_build_in_flight`). Called ONLY from Phase C/D's
    /// catch-up loop (`TableManager::phase_c_d_catchup_and_publish`, a
    /// different crate) — every other write path routes through
    /// plan_record_created/updated/deleted, which correctly captures to the
    /// dirty-set while a build is in-flight; calling those here would just
    /// re-capture instead of writing.
    ///
    /// `deltas`: one `(record_id, value_at_pin, value_now)` triple per drained
    /// dirty-set id. `value_at_pin`/`value_now` are `None` when the row did not
    /// exist at that version (pin: Phase A's scan never saw it and wrote no
    /// posting for it; now: the row is deleted).
    pub async fn apply_catchup_batch(
        &self,
        name_interned: u64,
        deltas: Vec<(RecordId, Option<InnerValue>, Option<InnerValue>)>,
    ) -> DbResult<()> {
        let Some(def) = self.indexes.get_index(name_interned) else {
            return Ok(());
        };

        let mut removes: Vec<RecordKey> = Vec::new();
        let mut sets: Vec<(RecordKey, Bytes)> = Vec::new();
        let mut cache_keys: Vec<Bytes> = Vec::new();

        for (record_id, old_value, new_value) in &deltas {
            let old_key = old_value
                .as_ref()
                .and_then(|v| build_index_key_from_record(false, name_interned, v, &def.paths))
                .map(|irk| irk.to_bytes());
            let new_key = new_value
                .as_ref()
                .and_then(|v| build_index_key_from_record(false, name_interned, v, &def.paths))
                .map(|irk| irk.to_bytes());

            if old_key == new_key {
                continue; // unchanged: both None, or same computed key
            }
            if let Some(ok) = &old_key {
                removes.push(build_posting_key(ok, record_id).into());
                cache_keys.push(ok.clone());
            }
            if let Some(nk) = &new_key {
                sets.push((build_posting_key(nk, record_id).into(), Bytes::new()));
                cache_keys.push(nk.clone());
            }
        }

        if !removes.is_empty() {
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.info_store.remove_many(removes).await?;
        }
        if !sets.is_empty() {
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.info_store.set_many(sets).await?;
        }
        for k in cache_keys {
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.posting_cache.remove(&k);
        }
        Ok(())
    }

    /// #1087: write a batch of postings during online build's Phase A backfill,
    /// and clear the posting cache for the affected index keys. Mirrors the
    /// inline batch-write step inside `create_index_from_stream`'s Phase 2 body
    /// (same `set_many` + `posting_cache.remove` pattern) — this is that same
    /// logic, exposed as a callable unit for the online-build orchestration
    /// living in `TableManager` (a different crate).
    pub async fn write_postings_batch(
        &self,
        posting_writes: Vec<(Bytes, Bytes)>,
        cache_index_keys: Vec<Bytes>,
    ) -> DbResult<()> {
        if !posting_writes.is_empty() {
            let posting_writes: Vec<(RecordKey, Bytes)> = posting_writes
                .into_iter()
                .map(|(k, v)| (k.into(), v))
                .collect();
            self.posting_cache_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            self.info_store.set_many(posting_writes).await?;
        }
        self.posting_cache_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        for ik in cache_index_keys {
            self.posting_cache.remove(&ik);
        }
        Ok(())
    }
}
