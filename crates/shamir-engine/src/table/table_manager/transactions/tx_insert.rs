use bytes::Bytes;
use shamir_collections::TFxSet;
use shamir_storage::error::DbResult;
use shamir_storage::types::{KvOp, RecordKey};
use shamir_tx::{IndexFamily, IndexWriteOp};
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::table_manager::TableManager;
use crate::index::index_definition::IndexDefinition
impl TableManager {
    /// tx-aware insert.
    ///
    /// - `tx == None` → delegates to existing `insert`.
    /// - `tx == Some` → stages data + index ops + counter delta in
    ///   TxContext. No physical writes. commit_tx Phase 5 applies.
    ///
    /// HIGH-6: base_index `IndexManager` (regular + unique) and
    /// `SortedIndexManager` posting writes ARE now staged into
    /// `tx.index_write_set` via `plan_base_index_insert_ops`. The planners
    /// emit `IndexWriteOp`s carrying the exact physical key layout the
    /// non-tx readers expect, so the commit pipeline applies them
    /// atomically and a dropped tx leaves no ghost postings. Unique-index
    /// validation runs at stage time below (read-only `validate_unique_for_create`)
    /// to reject duplicates early.
    ///
    /// tx-concurrent unique violation: stage-time validation reads
    /// committed state only, so two concurrent txs inserting the same
    /// unique value both pass it. The hole is now closed by recording a
    /// `UniqueGuard` per claimed unique key here; `commit_tx` Phase 2.6
    /// re-validates each guard under `commit_lock` (the same
    /// serialisation point the non-tx path gets from `unique_write_lock`).
    /// The unique key is deterministic in the value, so the commit-time
    /// `info_store.get(index_key)` settles ownership byte-for-byte.
    ///
    /// HIGH-6: stateful HNSW vectors are routed tx-locally — the index2
    /// `plan_insert_tx` is a no-op on the live graph for a tx, and
    /// `stage_vectors` buffers the embedding in `tx.staged_vectors`.
    /// Stateless peers (FTS / functional / btree) emit `IndexWriteOp`s
    /// accumulated in `tx.index_write_set`. A successful commit applies
    /// both (`commit_tx` Phase 5c for postings, Phase 5d for vectors); a
    /// dropped tx discards both by RAII.
    pub async fn insert_tx(
        &self,
        value: &InnerValue,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<RecordId> {
        let Some(tx) = tx else {
            return self.insert(value).await;
        };

        let rid = RecordId::new();

        // Level-3: acquire an Exclusive lock on the new record's key before
        // staging. No-op for Snapshot / Serializable (self-gates). The lock
        // is on the FUTURE key (the rid is fresh) so this never blocks on
        // existing data, but it serializes against a concurrent tx that
        // might read-then-write the same freshly-allocated rid.
        self.acquire_pessimistic_write_lock(RecordKey::from_slice(rid.as_bytes()), tx)
            .await?;

        // P0-2 (#958) / TOCTOU fix (2b): capture ALL three generation counters
        // BEFORE any `all_backends()` / `iter()` snapshot, and — #1098 — BEFORE
        // the `has_unique_indexes()`-gated validation/guard-recording block
        // below. Reading generation FIRST (Acquire) then snapshotting
        // guarantees (via Release-Acquire on each manager's generation
        // atomic, combined with each writer's publish-before-bump ordering —
        // `create_unique_index_from_records`/`drop_unique_index` set/clear
        // `UNIQUE_INDEX_EXISTS` BEFORE bumping generation, #1098 round 2) that
        // any backend/def published before the gen load is visible in the
        // snapshot AND in the later `has_unique_indexes()`/`unique_keys_for`
        // reads, and any published after the gen load but before those later
        // reads is double-planned (idempotent, harmless) or caught by the
        // commit-time `stage_gen != mgr.generation()` rederive gate. The OLD
        // order (checks/snapshot → gen capture, #1098) had a TOCTOU window
        // where a concurrent `CREATE UNIQUE INDEX` could bump generation
        // between the checks (seeing the old, no-index state) and the gen
        // capture (seeing the already-bumped value) — the commit-time gate
        // then saw no change and skipped re-derivation, silently admitting a
        // duplicate. #1098 round 2 ALSO required fixing the writer's own
        // publish order (flag-then-gen, not gen-then-flag) — with the flag
        // published second, a reader that observes the new generation is
        // guaranteed by the resulting happens-before edge to also observe
        // the flag; the reader-side reorder alone was necessary but not
        // sufficient.
        let token = self.table_token();
        let index2_gen = self.index2_registry().generation();
        let sorted_gen = self.sorted_indexes.generation();
        let base_index_gen = self.index_manager.generation();

        // #1098 test-only pause seam: fires strictly AFTER the generation
        // capture above and BEFORE the unique-check block below — see
        // `fire_post_gen_capture_pre_unique_check_test_hook`'s doc. No-op
        // in every non-test build.
        fire_post_gen_capture_pre_unique_check_test_hook().await;

        // HIGH-6: stage-time unique validation (read-only against
        // committed state). Optimistic fast-reject for the common
        // single-writer duplicate; the tx-concurrent case is settled by
        // the commit-time guard below.
        //
        // #1096: tx-aware — check if a conflicting durable key was released
        // earlier in this same tx (via DELETE or UPDATE-off), in which case
        // it's safe to reclaim. The on-demand `is_record_touched` probe closes
        // the stale-snapshot bypass — see `validate_unique_for_create_with_released`'s doc.
        //
        // `@oh` review (dbc6299e follow-up): gate behind `has_unique_indexes()`
        // — the walks themselves cost O(tx) regardless of the early-return
        // inside `validate_unique_for_create_with_released`.
        if self.index_manager.has_unique_indexes() {
            refresh_released_unique_cache(tx, self.table_token());
            let released = &tx.released_unique_cache[&self.table_token()].released;
            let table_token = self.table_token();
            let is_record_touched = |rid: &shamir_types::types::record_id::RecordId| -> bool {
                tx.write_set
                    .get(&table_token)
                    .is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
            };
            self.index_manager
                .validate_unique_for_create_with_released(value, released, is_record_touched)
                .await?;
        }

        // Record a UniqueGuard per unique key this value claims, so
        // commit_tx Phase 2.6 re-validates it under commit_lock (closes
        // the two-concurrent-txs hole). The recorded key is byte-identical
        // to what check_unique_constraint reads at commit time.
        for index_key in self.index_manager.unique_keys_for(value) {
            tx.record_unique_guard(shamir_tx::UniqueGuard {
                table_token: self.table_token(),
                index_key,
                owner: rid,
            });
        }

        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;

        // L9 fast-path: skip index planning entirely when the table has
        // no indexes. The `has_any_index()` check is O(1).

        let mut index_ops = Vec::new();
        if self.has_any_index() {
            let tx_id = Some(tx.tx_id);
            index_ops = self.plan_insert_ops(rid, value, tx_id).await?;
            index_ops.extend(self.plan_base_index_insert_ops(rid, value).await?);

            // HIGH-6: stage HNSW vectors tx-locally (not into the live graph).
            self.stage_vectors(rid, value, tx).await;
        }

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(RecordKey::from_slice(rid.as_bytes()), bytes),
                index_ops,
                counter_delta: 1,
            },
            tx,
        )
        .await?;

        // F-50 (#869) + F-50 Step 2 (#870) + P0-2 (#958): capture this table's
        // index2 / sorted / base_index generation (values read ABOVE, before the
        // stage-time snapshots) so the commit-time re-derivation gate can
        // detect a backend/def registered between this stage and commit.
        tx.note_index2_stage_gen(token, index2_gen);
        tx.note_sorted_stage_gen(token, sorted_gen);
        tx.note_base_index_stage_gen(token, base_index_gen);

        Ok(rid)
    }

    /// Batched tx-aware insert — mirrors `insert_many` for the tx
    /// staging path. Stages N records' data + index ops + counter
    /// delta into `tx` in one pass, lifting the per-row overhead
    /// (`validate_unique_for_create`, `unique_keys_for`,
    /// `all_backends().await` snapshots, `plan_base_index_insert_ops`)
    /// out of the row loop.
    ///
    /// Semantics MUST match calling `insert_tx` N times:
    ///   * each row gets a fresh `RecordId` (returned in input order);
    ///   * `UniqueGuard`s are recorded one-per-claim-per-row (the
    ///     `owner` is the row's rid — commit_tx Phase 2.6 needs that
    ///     to settle ownership);
    ///   * HNSW vectors are staged tx-locally via `stage_vector`;
    ///   * stateless index2 backends emit ops via `plan_insert_tx`;
    ///   * base_index / unique / sorted indexes emit ops via the existing
    ///     batch planners (`plan_records_created_batch`,
    ///     `plan_records_created_unique_batch`, sorted-by-def loop);
    ///   * counter delta = +N is bumped once;
    ///   * all per-row data writes go through one `ensure_table_staging`
    ///     handle via a single `staging.set_many` call — one synchronous
    ///     pass, no async overhead per key.
    ///
    /// Returns the assigned ids in input order. Empty input returns
    /// an empty Vec without touching `tx`.
    pub async fn insert_tx_many(
        &self,
        values: &[InnerValue],
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<Vec<RecordId>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        // P0-2 (#958) / TOCTOU fix (2b): capture generations BEFORE any
        // `all_backends()` / `iter()` snapshot (see `insert_tx`'s comment).
        let token = self.table_token();
        let index2_gen = self.index2_registry().generation();
        let sorted_gen = self.sorted_indexes.generation();
        let base_index_gen = self.index_manager.generation();

        // 1. Batch-validate unique indexes. Mirrors `insert_many`:
        //    persisted check + batch-local seen set (so two rows in
        //    ONE batch claiming the same unique value reject the
        //    later one rather than silently overwriting).
        //
        // #1096: tx-aware — also check for keys released by earlier
        //    staged ops (DELETE or UPDATE-off) in the same tx. The on-demand
        //    `is_record_touched` probe closes the stale-snapshot bypass.
        if self.index_manager.has_unique_indexes() {
            // Snapshot unique-index defs ONCE per batch — stable for the
            // duration of insert_tx_many (mutated only by DDL, which cannot
            // run concurrently with this call). Mirrors the non-tx
            // `insert_many_returning_version` hoist (`table_manager_crud.rs`)
            // — this was previously re-collected (DashMap-iter + N×
            // `IndexDefinition::clone`) on EVERY row via
            // `iter_unique_indexes()` inside the loop below.
            let unique_defs: Vec<IndexDefinition> =
                self.index_manager.iter_unique_indexes().collect();
            let mut batch_seen: TFxSet<(u64, Vec<u8>)> = TFxSet::default();
            refresh_released_unique_cache(tx, self.table_token());
            let released = &tx.released_unique_cache[&self.table_token()].released;
            let table_token = self.table_token();
            let is_record_touched = |rid: &shamir_types::types::record_id::RecordId| -> bool {
                tx.write_set
                    .get(&table_token)
                    .is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
            };
            for (i, v) in values.iter().enumerate() {
                self.index_manager
                    .validate_unique_for_create_with_released(v, released, &is_record_touched)
                    .await?;
                for def in &unique_defs {
                    if let Some(vs) = crate::index::index_keys::extract_index_leaves(v, &def.paths)
                    {
                        let key = bincode::serialize(&vs)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        if !batch_seen.insert((def.name_interned, key)) {
                            return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                                "Unique index '{}' violated within batch (row {} duplicates an earlier row)",
                                def.name_interned, i
                            )));
                        }
                    }
                }
            }
        }

        // 2. Generate ids upfront. Serialization (to_bytes) is deferred to
        //    commit Phase 4/5 via StagedRow::Live — aborted txs skip it.
        //    L13: single clock-read for the whole batch; intra-batch
        //    monotonicity via ascending seq in `from_ts_seq`.
        let batch_ts = RecordId::now_micros();
        let mut ids: Vec<RecordId> = Vec::with_capacity(values.len());
        for (i, _) in values.iter().enumerate() {
            ids.push(RecordId::from_ts_seq(batch_ts, i as u32));
        }

        // 2b. Precompute Bytes keys once — reused in the lock loop and
        //    set_many_live below.
        let id_bytes: Vec<bytes::Bytes> = ids.iter().map(|rid| rid.to_bytes()).collect();

        // Level-3: acquire Exclusive locks on every new rid before
        // staging. No-op for Snapshot / Serializable (self-gates). task #532:
        // the lock registry is `RecordKey`-keyed; `id_bytes` is reused as
        // `Bytes` by `set_many_live` below, so convert per lock call
        // (inline-cheap for the 16-byte rid shape).
        for key in &id_bytes {
            self.acquire_pessimistic_write_lock(RecordKey::from(key.clone()), tx)
                .await?;
        }

        // 3. Record UniqueGuards per row per unique index it claims.
        //    Same shape as `insert_tx` (one guard per claimed key,
        //    `owner = rid`) — commit_tx Phase 2.6 settles ownership
        //    per guard, byte-identical to the per-row staging path.
        if self.index_manager.has_unique_indexes() {
            let token = self.table_token();
            for (rid, v) in ids.iter().zip(values.iter()) {
                for index_key in self.index_manager.unique_keys_for(v) {
                    tx.record_unique_guard(shamir_tx::UniqueGuard {
                        table_token: token,
                        index_key,
                        owner: *rid,
                    });
                }
            }
        }

        // 4–5. Index planning — skipped entirely when the table has no
        //    indexes (L9 fast-path). The `has_any_index()` check is O(1)
        //    (atomic loads + `is_empty` on lock-free maps).
        let mut index_ops: Vec<shamir_tx::IndexWriteOp> = Vec::new();
        if self.has_any_index() {
            // 4. Take the index2 backend snapshot ONCE, then drive both
            //    plan_insert_tx (stateless ops → index_write_set) and
            //    staged_vector (HNSW → tx.staged_vectors) per row off the
            //    cached list. This is the main per-row→batched lift:
            //    `all_backends().await` walks the scc::HashMap; doing it
            //    once amortises across N rows.
            let backends = self.index2_registry.all_backends().await;
            let tx_id = Some(tx.tx_id);
            for (rid, v) in ids.iter().zip(values.iter()) {
                for backend in &backends {
                    let mut ops = backend
                        .plan_insert_tx(*rid, v, tx_id)
                        .await
                        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
                    // F-1 (#1027): this inlined loop (kept inline to amortize
                    // the `all_backends()` snapshot across the whole batch)
                    // must stamp the REAL live instance epoch exactly like
                    // `plan_insert_ops`/`plan_update_ops`/`plan_delete_ops`
                    // do — an unstamped op carries the placeholder
                    // `instance_epoch: 0` and gets silently retracted by the
                    // commit-time reconcile the moment ANY index2 DDL bumps
                    // the registry generation between stage and commit (see
                    // `stamp_index2_provenance`'s doc).
                    self.stamp_index2_provenance(backend, &mut ops).await;
                    index_ops.extend(ops);
                    if let Some(vec) = backend.staged_vector(*rid, v).await {
                        tx.stage_vector(token, *rid, vec);
                    }
                }
            }

            // 5. base_index + sorted batch planners — one call each, planning
            //    over the whole (id, value) iterator. Same physical key
            //    layout the non-tx readers expect (see
            //    `plan_base_index_insert_ops` for the contract).
            let pairs = || ids.iter().zip(values.iter());
            let mut base_index_ops = self
                .index_manager
                .plan_records_created_batch(pairs())
                .await?;
            base_index_ops.extend(
                self.index_manager
                    .plan_records_created_unique_batch(pairs())
                    .await?,
            );
            base_index_ops.extend(self.sorted_indexes.plan_records_created_batch(pairs(), 0)?);
            index_ops.extend(base_index_ops);
        }

        // 6. Single ensure_table_staging, then set_many: one synchronous
        //    pass — no async overhead per key. InnerValues are serialized
        //    to bytes here (the Live variant was removed in W2c).
        let staging = tx.ensure_table_staging(token, &self.name, self.table.data_store().clone());
        let staged_bytes: Vec<Bytes> = {
            let mut out = Vec::with_capacity(values.len());
            for v in values {
                out.push(v.to_bytes().map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "Failed to serialize InnerValue: {}",
                        e
                    ))
                })?);
            }
            out
        };
        staging.set_many(
            id_bytes
                .into_iter()
                .zip(staged_bytes)
                .map(|(k, v)| (k.into(), v)),
        );

        // 7. Merge index_ops + counter delta in one go.
        tx.index_write_set
            .extend(index_ops.into_iter().map(|op| (token, op)));
        tx.bump_counter(token, values.len() as i64);

        // F-50 (#869) + F-50 Step 2 (#870) + P0-2 (#958): capture the index2 /
        // sorted / base_index generation (values read ABOVE, before the stage-time
        // snapshots) to gate commit-time re-derivation.
        tx.note_index2_stage_gen(token, index2_gen);
        tx.note_sorted_stage_gen(token, sorted_gen);
        tx.note_base_index_stage_gen(token, base_index_gen);

        Ok(ids)
    }

    /// W2d — lens-driven batched tx insert. Structurally identical to
    /// `insert_tx_many` but takes already-encoded storage `Bytes` (produced
    /// by `query_value_to_storage_bytes` in `execute_insert_tx`) instead of
    /// `InnerValue` trees. Index extraction, unique validation, vector
    /// staging, and all posting planners run through `RecordView` (zero-copy
    /// lens over the msgpack bytes) — no `InnerValue` tree is ever built on
    /// the insert path.
    ///
    /// INVARIANT (Dec/Big/Set guard): `staged[i]` was produced by
    /// `query_value_to_storage_bytes`, which encodes Dec/Big/Set as
    /// `serialize_str` (msgpack `str`), and the source `QueryValue` from
    /// QueryValue / `resolve_computed_record` never yields Dec/Big/Set variants.
    /// Therefore both the tree path (via `InnerValue`) and the lens path
    /// (via `RecordView`) see the SAME `Str` for what used to be Dec/Big,
    /// so index-key extraction agrees byte-for-byte. If a future msgpack-client
    /// introduces a real Dec/Big `QueryValue` source, a Dec/Big-keyed index
    /// would diverge under the lens — gate that with a debug-assert / typed
    /// encoder before extending this path.
    pub async fn insert_tx_many_bytes(
        &self,
        staged: &[Bytes],
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<Vec<RecordId>> {
        if staged.is_empty() {
            return Ok(Vec::new());
        }

        // Build one RecordView per staged row — the zero-copy lens that feeds
        // every RecordRef-accepting extractor below. `RecordView::new`
        // validates the top-level map header only (no tree decode).
        let views: Vec<shamir_types::record_view::RecordView<'_>> = staged
            .iter()
            .map(|b| shamir_types::record_view::RecordView::new(b))
            .collect::<Result<_, _>>()
            .map_err(|e| shamir_storage::error::DbError::Codec(format!("RecordView: {e}")))?;

        // P0-2 (#958) / TOCTOU fix (2b): capture generations BEFORE the
        // `all_backends()` snapshot below.
        let token = self.table_token();
        let index2_gen = self.index2_registry().generation();
        let sorted_gen = self.sorted_indexes.generation();
        let base_index_gen = self.index_manager.generation();

        // 1. Batch-validate unique indexes — same shape as `insert_tx_many`
        //    but driven through the lens (`&views[i]` as `&impl RecordRef`).
        //
        // #1096 follow-up (found by `@oh` review): this is the path
        // `execute_insert_tx`/`execute_set_tx` actually call for every
        // wire INSERT/UPSERT (see the F-1 comment on `insert_tx_many_bytes`'s
        // index2-planning loop below) — it must get the SAME tx-aware
        // released-key + touched-record treatment `insert_tx`/`insert_tx_many` have.
        // The on-demand `is_record_touched` probe closes the stale-snapshot bypass.
        if self.index_manager.has_unique_indexes() {
            // Snapshot unique-index defs ONCE per batch — see
            // `insert_tx_many`'s identical hoist for the full rationale.
            let unique_defs: Vec<IndexDefinition> =
                self.index_manager.iter_unique_indexes().collect();
            let mut batch_seen: TFxSet<(u64, Vec<u8>)> = TFxSet::default();
            refresh_released_unique_cache(tx, self.table_token());
            let released = &tx.released_unique_cache[&self.table_token()].released;
            let table_token = self.table_token();
            let is_record_touched = |rid: &shamir_types::types::record_id::RecordId| -> bool {
                tx.write_set
                    .get(&table_token)
                    .is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
            };
            for (i, view) in views.iter().enumerate() {
                self.index_manager
                    .validate_unique_for_create_with_released(view, released, &is_record_touched)
                    .await?;
                for def in &unique_defs {
                    if let Some(vs) =
                        crate::index::index_keys::extract_index_leaves(view, &def.paths)
                    {
                        let key = bincode::serialize(&vs)
                            .map_err(|e| shamir_storage::error::DbError::Codec(e.to_string()))?;
                        if !batch_seen.insert((def.name_interned, key)) {
                            return Err(shamir_storage::error::DbError::DuplicateKey(format!(
                                "Unique index '{}' violated within batch (row {} duplicates an earlier row)",
                                def.name_interned, i
                            )));
                        }
                    }
                }
            }
        }

        // 2. Generate ids upfront.
        //    L13: single clock-read for the whole batch; intra-batch
        //    monotonicity via ascending seq in `from_ts_seq`.
        let batch_ts = RecordId::now_micros();
        let mut ids: Vec<RecordId> = Vec::with_capacity(staged.len());
        for (i, _) in staged.iter().enumerate() {
            ids.push(RecordId::from_ts_seq(batch_ts, i as u32));
        }

        // 2b. Precompute Bytes keys once — reused in the lock loop and set_many.
        let id_bytes: Vec<Bytes> = ids.iter().map(|rid| rid.to_bytes()).collect();

        // Level-3: acquire Exclusive locks on every new rid before staging.
        // task #532: the lock registry is `RecordKey`-keyed; `id_bytes` is
        // reused as `Bytes` by `set_many` below, so convert per lock call
        // (inline-cheap for the 16-byte rid shape).
        for key in &id_bytes {
            self.acquire_pessimistic_write_lock(RecordKey::from(key.clone()), tx)
                .await?;
        }

        // 3. Record UniqueGuards per row per unique index it claims.
        if self.index_manager.has_unique_indexes() {
            let token = self.table_token();
            for (rid, view) in ids.iter().zip(views.iter()) {
                for index_key in self.index_manager.unique_keys_for(view) {
                    tx.record_unique_guard(shamir_tx::UniqueGuard {
                        table_token: token,
                        index_key,
                        owner: *rid,
                    });
                }
            }
        }

        // 4–5. Index planning — skipped entirely when the table has no
        //    indexes (L9 fast-path). The `has_any_index()` check is O(1)
        //    (atomic loads + `is_empty` on lock-free maps).
        let mut index_ops: Vec<shamir_tx::IndexWriteOp> = Vec::new();
        if self.has_any_index() {
            // 4. Take the index2 backend snapshot ONCE, then drive both
            //    plan_insert_tx (stateless ops → index_write_set) and
            //    staged_vector (HNSW → tx.staged_vectors) per row off the
            //    cached list, feeding `&views[i]` as `&dyn RecordRef`.
            let backends = self.index2_registry.all_backends().await;
            let tx_id = Some(tx.tx_id);
            for (rid, view) in ids.iter().zip(views.iter()) {
                for backend in &backends {
                    let mut ops = backend
                        .plan_insert_tx(*rid, view, tx_id)
                        .await
                        .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
                    // F-1 (#1027): see the identical comment in
                    // `insert_tx_many` — this inlined loop must stamp the
                    // REAL live instance epoch or the op is silently
                    // retracted by the commit-time reconcile the moment ANY
                    // index2 DDL bumps the registry generation between stage
                    // and commit. `insert_tx_many_bytes` is not a rare path:
                    // it's what `execute_insert_tx`/`execute_set_tx` call for
                    // every transactional INSERT/UPSERT, including a single
                    // row.
                    self.stamp_index2_provenance(backend, &mut ops).await;
                    index_ops.extend(ops);
                    if let Some(vec) = backend.staged_vector(*rid, view).await {
                        tx.stage_vector(token, *rid, vec);
                    }
                }
            }

            // 5. base_index + sorted batch planners — one call each, planning
            //    over the whole (id, view) iterator. `RecordView: RecordRef`,
            //    so the generic `R: RecordRef` bound is satisfied.
            let pairs = || ids.iter().zip(views.iter());
            let mut base_index_ops = self
                .index_manager
                .plan_records_created_batch(pairs())
                .await?;
            base_index_ops.extend(
                self.index_manager
                    .plan_records_created_unique_batch(pairs())
                    .await?,
            );
            base_index_ops.extend(self.sorted_indexes.plan_records_created_batch(pairs(), 0)?);
            index_ops.extend(base_index_ops);
        }

        // 6. Single ensure_table_staging, then set_many with the staged
        //    bytes (NOT set_many_live — bytes are already encoded).
        let staging = tx.ensure_table_staging(token, &self.name, self.table.data_store().clone());
        staging.set_many(
            id_bytes
                .into_iter()
                .zip(staged.iter().cloned())
                .map(|(k, v)| (k.into(), v)),
        );

        // 7. Merge index_ops + counter delta in one go.
        tx.index_write_set
            .extend(index_ops.into_iter().map(|op| (token, op)));
        tx.bump_counter(token, staged.len() as i64);

        // F-50 (#869) + F-50 Step 2 (#870) + P0-2 (#958): capture the index2 /
        // sorted / base_index generation (values read ABOVE, before the stage-time
        // snapshots) to gate commit-time re-derivation.
        tx.note_index2_stage_gen(token, index2_gen);
        tx.note_sorted_stage_gen(token, sorted_gen);
        tx.note_base_index_stage_gen(token, base_index_gen);

        Ok(ids)
    }
}
