use shamir_storage::error::DbResult;
use shamir_storage::types::KvOp;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;

/// Bundled mutation effect for one record. Built by *_tx methods,
/// applied by `stage_mutation` to a TxContext.
pub(super) struct StagedMutation {
    pub(super) data_op: KvOp,
    pub(super) index_ops: Vec<shamir_tx::IndexWriteOp>,
    pub(super) counter_delta: i64,
}

impl TableManager {
    /// Apply a staged mutation to the TxContext.
    pub(super) async fn stage_mutation(
        &self,
        m: StagedMutation,
        tx: &mut shamir_tx::TxContext,
    ) -> DbResult<()> {
        let staging = tx.ensure_table_staging(
            self.table_token(),
            &self.name,
            self.table.data_store().clone(),
        );
        match m.data_op {
            KvOp::Set(k, v) => staging.set(k, v),
            KvOp::Remove(k) => staging.remove(k),
        }
        let token = self.table_token();
        tx.index_write_set
            .extend(m.index_ops.into_iter().map(|op| (token, op)));
        tx.bump_counter(self.table_token(), m.counter_delta);
        Ok(())
    }

    /// Collect index ops from all index2 backends for an insert.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// `tx_id` is forwarded to each backend's `plan_insert_tx` so
    /// backends that maintain non-storage state (e.g. `VectorBackend`'s
    /// HNSW graph) can route the mutation into a per-tx staging area
    /// instead of the live structure (HIGH-6). Stateless backends
    /// (FTS / functional / btree) fall through to `plan_insert` via
    /// the default trait impl.
    pub(super) async fn plan_insert_ops(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Vec<shamir_tx::IndexWriteOp> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_insert_tx(rid, rec, tx_id).await {
                all_ops.extend(ops);
            }
        }
        all_ops
    }

    /// HIGH-6: route any HNSW vectors carried by `rec` into the tx's own
    /// `staged_vectors` buffer instead of the live graph. Each vector
    /// backend extracts its embedding (`IndexBackend::staged_vector`);
    /// the `(rid, vec)` pair lands under this table's token. Promoted
    /// into the graph atomically at commit (Phase 5d), discarded by RAII
    /// on abort. Stateless backends return `None` and contribute nothing.
    ///
    /// The tx-aware `plan_*_tx` methods deliberately leave the live graph
    /// untouched (no-op for `Some(tx)`), so this is the sole staging path.
    pub(super) async fn stage_vectors(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx: &mut shamir_tx::TxContext,
    ) {
        let token = self.table_token();
        for backend in self.index2_registry.all_backends().await {
            if let Some(v) = backend.staged_vector(rid, rec).await {
                tx.stage_vector(token, rid, v);
            }
        }
    }

    /// Collect index ops from all index2 backends for an update.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// See [`plan_insert_ops`] for the `tx_id` parameter.
    pub(super) async fn plan_update_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Vec<shamir_tx::IndexWriteOp> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_update_tx(rid, old, new, tx_id).await {
                all_ops.extend(ops);
            }
        }
        all_ops
    }

    /// Collect index ops from all index2 backends for a delete.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// See [`plan_insert_ops`] for the `tx_id` parameter.
    pub(super) async fn plan_delete_ops(
        &self,
        rid: RecordId,
        rec: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> Vec<shamir_tx::IndexWriteOp> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            if let Ok(ops) = backend.plan_delete_tx(rid, rec, tx_id).await {
                all_ops.extend(ops);
            }
        }
        all_ops
    }

    /// HIGH-6: collect legacy `IndexManager` (regular + unique) and
    /// `SortedIndexManager` posting ops for a tx insert. These ops use
    /// the *exact* physical key layout the non-tx readers expect
    /// (`lookup_by_index` / `check_unique_constraint` / `lookup_range`),
    /// so applying them at commit time produces postings indistinguishable
    /// from the non-tx `on_record_created` path. The unique ops do NOT
    /// validate — validation runs separately at stage time in `insert_tx`.
    pub(super) async fn plan_legacy_insert_ops(
        &self,
        rid: RecordId,
        rec: &InnerValue,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut ops = self.index_manager.plan_record_created(&rid, rec).await?;
        ops.extend(
            self.index_manager
                .plan_record_created_unique(&rid, rec)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_created(&rid, rec, 0)?);
        Ok(ops)
    }

    /// HIGH-6: legacy + sorted posting ops for a tx update.
    pub(super) async fn plan_legacy_update_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut ops = self
            .index_manager
            .plan_record_updated(&rid, old, new)
            .await?;
        ops.extend(
            self.index_manager
                .plan_record_updated_unique(&rid, old, new)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_updated(&rid, old, new, 0)?);
        Ok(ops)
    }

    /// HIGH-6: legacy + sorted posting ops for a tx delete.
    pub(super) async fn plan_legacy_delete_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut ops = self.index_manager.plan_record_deleted(&rid, old).await?;
        ops.extend(
            self.index_manager
                .plan_record_deleted_unique(&rid, old)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_deleted(&rid, old)?);
        Ok(ops)
    }

    /// tx-aware insert.
    ///
    /// - `tx == None` → delegates to existing [`insert`].
    /// - `tx == Some` → stages data + index ops + counter delta in
    ///   TxContext. No physical writes. commit_tx Phase 5 applies.
    ///
    /// HIGH-6: legacy `IndexManager` (regular + unique) and
    /// `SortedIndexManager` posting writes ARE now staged into
    /// `tx.index_write_set` via [`plan_legacy_insert_ops`]. The planners
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
    /// [`stage_vectors`] buffers the embedding in `tx.staged_vectors`.
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
        self.acquire_pessimistic_write_lock(rid.to_bytes(), tx)
            .await?;

        // HIGH-6: stage-time unique validation (read-only against
        // committed state). Optimistic fast-reject for the common
        // single-writer duplicate; the tx-concurrent case is settled by
        // the commit-time guard below.
        self.index_manager.validate_unique_for_create(value).await?;

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

        let tx_id = Some(tx.tx_id);
        let mut index_ops = self.plan_insert_ops(rid, value, tx_id).await;
        index_ops.extend(self.plan_legacy_insert_ops(rid, value).await?);

        // HIGH-6: stage HNSW vectors tx-locally (not into the live graph).
        self.stage_vectors(rid, value, tx).await;

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(rid.to_bytes(), bytes),
                index_ops,
                counter_delta: 1,
            },
            tx,
        )
        .await?;

        Ok(rid)
    }

    /// Batched tx-aware insert — mirrors [`insert_many`] for the tx
    /// staging path. Stages N records' data + index ops + counter
    /// delta into `tx` in one pass, lifting the per-row overhead
    /// (`validate_unique_for_create`, `unique_keys_for`,
    /// `all_backends().await` snapshots, `plan_legacy_insert_ops`)
    /// out of the row loop.
    ///
    /// Semantics MUST match calling [`insert_tx`] N times:
    ///   * each row gets a fresh `RecordId` (returned in input order);
    ///   * `UniqueGuard`s are recorded one-per-claim-per-row (the
    ///     `owner` is the row's rid — commit_tx Phase 2.6 needs that
    ///     to settle ownership);
    ///   * HNSW vectors are staged tx-locally via [`stage_vector`];
    ///   * stateless index2 backends emit ops via `plan_insert_tx`;
    ///   * legacy / unique / sorted indexes emit ops via the existing
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

        // 1. Batch-validate unique indexes. Mirrors `insert_many`:
        //    persisted check + batch-local seen set (so two rows in
        //    ONE batch claiming the same unique value reject the
        //    later one rather than silently overwriting).
        if self.index_manager.has_unique_indexes() {
            let mut batch_seen: std::collections::HashSet<(u64, Vec<u8>), THasher> =
                std::collections::HashSet::default();
            for (i, v) in values.iter().enumerate() {
                self.index_manager.validate_unique_for_create(v).await?;
                for def in self.index_manager.iter_unique_indexes() {
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
        let mut ids: Vec<RecordId> = Vec::with_capacity(values.len());
        for _ in values {
            ids.push(RecordId::new());
        }

        // 2b. Precompute Bytes keys once — reused in the lock loop and
        //    set_many_live below.
        let id_bytes: Vec<bytes::Bytes> = ids.iter().map(|rid| rid.to_bytes()).collect();

        // Level-3: acquire Exclusive locks on every new rid before
        // staging. No-op for Snapshot / Serializable (self-gates).
        for key in &id_bytes {
            self.acquire_pessimistic_write_lock(key.clone(), tx).await?;
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

        // 4. Take the index2 backend snapshot ONCE, then drive both
        //    plan_insert_tx (stateless ops → index_write_set) and
        //    staged_vector (HNSW → tx.staged_vectors) per row off the
        //    cached list. This is the main per-row→batched lift:
        //    `all_backends().await` walks the scc::HashMap; doing it
        //    once amortises across N rows.
        let backends = self.index2_registry.all_backends().await;
        let tx_id = Some(tx.tx_id);
        let token = self.table_token();
        let mut index_ops: Vec<shamir_tx::IndexWriteOp> = Vec::new();
        for (rid, v) in ids.iter().zip(values.iter()) {
            for backend in &backends {
                if let Ok(ops) = backend.plan_insert_tx(*rid, v, tx_id).await {
                    index_ops.extend(ops);
                }
                if let Some(vec) = backend.staged_vector(*rid, v).await {
                    tx.stage_vector(token, *rid, vec);
                }
            }
        }

        // 5. Legacy + sorted batch planners — one call each, planning
        //    over the whole (id, value) iterator. Same physical key
        //    layout the non-tx readers expect (see
        //    `plan_legacy_insert_ops` for the contract).
        let pairs = || ids.iter().zip(values.iter());
        let mut legacy_ops = self
            .index_manager
            .plan_records_created_batch(pairs())
            .await?;
        legacy_ops.extend(
            self.index_manager
                .plan_records_created_unique_batch(pairs())
                .await?,
        );
        legacy_ops.extend(self.sorted_indexes.plan_records_created_batch(pairs(), 0)?);
        index_ops.extend(legacy_ops);

        // 6. Single ensure_table_staging, then set_many_live: one synchronous
        //    pass — no async overhead per key. InnerValues are stored as
        //    StagedRow::Live; msgpack encoding is deferred to commit Phase 4/5.
        //    Aborted txs skip encoding entirely.
        let staging = tx.ensure_table_staging(token, &self.name, self.table.data_store().clone());
        staging.set_many_live(id_bytes.into_iter().zip(values.iter().cloned()));

        // 7. Merge index_ops + counter delta in one go.
        tx.index_write_set
            .extend(index_ops.into_iter().map(|op| (token, op)));
        tx.bump_counter(token, values.len() as i64);

        Ok(ids)
    }

    /// tx-aware update.
    ///
    /// `tx == None` → existing `set` (since `update` is currently
    /// internal helper; `set` is the public surface).
    /// `tx == Some` → reads old value via `read_one_tx` (write_set or
    /// main store), plans diff index ops via `plan_update`, stages
    /// the new bytes.
    ///
    /// Returns `true` if a record was already present (semantically
    /// matches existing `set`).
    ///
    /// HIGH-6: see `insert_tx` for the staging contract and the
    /// commit-time application gap.
    pub async fn update_tx(
        &self,
        id: RecordId,
        value: &InnerValue,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        let Some(tx) = tx else {
            return self.set(id, value).await;
        };

        let old = self.read_one_tx(id, Some(&*tx)).await.ok();

        // Level-3: acquire an Exclusive lock on the key before staging the
        // write. `read_one_tx` above already took a Shared lock for a
        // Pessimistic tx; this re-entrant acquire upgrades it to Exclusive
        // (same tx — never self-deadlocks). No-op for Snapshot / Serializable.
        self.acquire_pessimistic_write_lock(id.to_bytes(), tx)
            .await?;

        // HIGH-6: stage-time unique validation (read-only). For an
        // existing record this excludes the record itself; for a fresh
        // insert it behaves like create-validation.
        match &old {
            Some(old_val) => {
                self.index_manager
                    .validate_unique_for_update(&id, old_val, value)
                    .await?
            }
            None => self.index_manager.validate_unique_for_create(value).await?,
        }

        // Record a UniqueGuard per unique key the NEW value claims, owner
        // = the rid being updated. commit_tx Phase 2.6 re-validates under
        // commit_lock; an update re-writing its own value sees
        // `existing == owner` and is not a self-conflict.
        for index_key in self.index_manager.unique_keys_for(value) {
            tx.record_unique_guard(shamir_tx::UniqueGuard {
                table_token: self.table_token(),
                index_key,
                owner: id,
            });
        }

        let bytes = value.to_bytes().map_err(|e| {
            shamir_storage::error::DbError::Codec(format!("Failed to serialize InnerValue: {}", e))
        })?;

        let tx_id = Some(tx.tx_id);
        let (mut index_ops, counter_delta) = match &old {
            Some(old_val) => (self.plan_update_ops(id, old_val, value, tx_id).await, 0_i64),
            None => (self.plan_insert_ops(id, value, tx_id).await, 1_i64),
        };
        match &old {
            Some(old_val) => {
                index_ops.extend(self.plan_legacy_update_ops(id, old_val, value).await?)
            }
            None => index_ops.extend(self.plan_legacy_insert_ops(id, value).await?),
        }

        // HIGH-6: stage the new vector tx-locally (apply_committed_vectors
        // upsert-replaces the prior committed entry at commit time).
        self.stage_vectors(id, value, tx).await;

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(id.to_bytes(), bytes),
                index_ops,
                counter_delta,
            },
            tx,
        )
        .await?;

        Ok(old.is_some())
    }

    /// tx-aware delete.
    ///
    /// `tx == None` → existing `delete`.
    /// `tx == Some` → reads old value, plans delete ops, stages
    /// Remove. Returns `true` if a record was present.
    ///
    /// HIGH-6: see `insert_tx` for the staging contract and the
    /// commit-time application gap.
    pub async fn delete_tx(
        &self,
        id: RecordId,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        let Some(tx) = tx else {
            return self.delete(id).await;
        };

        let Some(old) = self.read_one_tx(id, Some(&*tx)).await.ok() else {
            return Ok(false);
        };

        // Level-3: acquire an Exclusive lock on the key before staging the
        // delete. Re-entrant upgrade from the Shared lock `read_one_tx` took.
        // No-op for Snapshot / Serializable.
        self.acquire_pessimistic_write_lock(id.to_bytes(), tx)
            .await?;

        let tx_id = Some(tx.tx_id);
        let mut index_ops = self.plan_delete_ops(id, &old, tx_id).await;
        index_ops.extend(self.plan_legacy_delete_ops(id, &old).await?);

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Remove(id.to_bytes()),
                index_ops,
                counter_delta: -1,
            },
            tx,
        )
        .await?;

        Ok(true)
    }

    /// tx-aware insert-or-update by RecordId. Alias of [`update_tx`]
    /// — same semantics in tx mode.
    pub async fn set_tx(
        &self,
        id: RecordId,
        value: &InnerValue,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        self.update_tx(id, value, tx).await
    }
}
