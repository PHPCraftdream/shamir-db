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

        // Only NotFound is absence; every other pre-read error fails closed.
        let old = match self.read_one_tx(id, Some(&*tx)).await {
            Ok(value) => Some(value),
            Err(shamir_storage::error::DbError::NotFound(_)) => None,
            Err(error) => return Err(error),
        };

        // Level-3: acquire an Exclusive lock on the key before staging the
        // write. `read_one_tx` above already took a Shared lock for a
        // Pessimistic tx; this re-entrant acquire upgrades it to Exclusive
        // (same tx — never self-deadlocks). No-op for Snapshot / Serializable.
        self.acquire_pessimistic_write_lock(RecordKey::from_slice(id.as_bytes()), tx)
            .await?;

        // P0-2 (#958) / TOCTOU fix (2b): capture generations BEFORE any
        // `all_backends()` / `iter()` snapshot (see `insert_tx`'s comment).
        let token = self.table_token();
        let index2_gen = self.index2_registry().generation();
        let sorted_gen = self.sorted_indexes.generation();
        let base_index_gen = self.index_manager.generation();

        // HIGH-6: stage-time unique validation (read-only). For an
        // existing record this excludes the record itself; for a fresh
        // insert it behaves like create-validation.
        //
        // #1096 follow-up (found by `@oh` review): tx-aware — an UPDATE can
        // be either half of a release-then-reclaim pair (the release, via
        // `validate_unique_for_update`'s old->new diff; or the reclaim, via
        // the `None` upsert-into-fresh-id branch), so both need the same
        // released-key + touched-record treatment `insert_tx` has. The on-demand
        // `is_record_touched` probe closes the stale-snapshot bypass.
        //
        // `@oh` review (dbc6299e follow-up): gate behind `has_unique_indexes()`
        // like `insert_tx_many`/`insert_tx_many_bytes` already do — computing
        // `released` unconditionally makes every UPDATE on a table with NO unique
        // indexes pay an O(|tx.index_write_set|) walk for nothing. This is
        // `update_tx`, the direct (non-per-row) API; the on-demand
        // `is_record_touched` probe (#1099) is what eliminates the O(N²) cost
        // on `update_tx_bytes`'s wire path (called per-row) by avoiding an
        // O(N) set build for every row there.
        if self.index_manager.has_unique_indexes() {
            refresh_released_unique_cache(tx, self.table_token());
            let released = &tx.released_unique_cache[&self.table_token()].released;
            let table_token = self.table_token();
            let is_record_touched = |rid: &shamir_types::types::record_id::RecordId| -> bool {
                tx.write_set
                    .get(&table_token)
                    .is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
            };
            match &old {
                Some(old_val) => {
                    self.index_manager
                        .validate_unique_for_update_with_released(
                            &id,
                            old_val,
                            value,
                            released,
                            &is_record_touched,
                        )
                        .await?
                }
                None => {
                    self.index_manager
                        .validate_unique_for_create_with_released(
                            value,
                            released,
                            &is_record_touched,
                        )
                        .await?
                }
            }
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
            Some(old_val) => (
                self.plan_update_ops(id, old_val, value, tx_id).await?,
                0_i64,
            ),
            None => (self.plan_insert_ops(id, value, tx_id).await?, 1_i64),
        };
        match &old {
            Some(old_val) => {
                index_ops.extend(self.plan_base_index_update_ops(id, old_val, value).await?)
            }
            None => index_ops.extend(self.plan_base_index_insert_ops(id, value).await?),
        }

        // HIGH-6: stage the new vector tx-locally (apply_committed_vectors
        // upsert-replaces the prior committed entry at commit time).
        self.stage_vectors(id, value, tx).await;
        // gap#1 (update branch): if the update REMOVES the embedding field,
        // stage a vector delete — otherwise the old vector stays live in the
        // graph as a ghost (the tx path has no non-tx `plan_update` else-
        // branch tombstone). See `stage_vector_deletes_on_update`.
        if let Some(old_val) = &old {
            self.stage_vector_deletes_on_update(id, old_val, value, tx)
                .await;
        }

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(RecordKey::from_slice(id.as_bytes()), bytes),
                index_ops,
                counter_delta,
            },
            tx,
        )
        .await?;

        // F-50 (#869) + F-50 Step 2 (#870) + P0-2 (#958): capture the index2 /
        // sorted / base_index generation (values read ABOVE, before the stage-time
        // snapshots).
        tx.note_index2_stage_gen(token, index2_gen);
        tx.note_sorted_stage_gen(token, sorted_gen);
        tx.note_base_index_stage_gen(token, base_index_gen);

        Ok(old.is_some())
    }

    /// Byte-level tx-aware update — the W3 counterpart of `update_tx`.
    ///
    /// Called by `execute_update_tx` after `merge_storage_bytes` has already
    /// produced `new_bytes` from `old_bytes + set_map`. Skips the
    /// `read_one_tx` round-trip (caller already has old_bytes from the scan)
    /// and drives index/unique/vector planners through zero-copy
    /// `RecordView` lenses instead of decoded `InnerValue` trees.
    ///
    /// The record MUST already exist (this is an update, not an upsert);
    /// `old_bytes` is the committed storage bytes the caller matched.
    ///
    /// `ids_captured` (#1205): pass `true` only when the caller already
    /// captured every `InternerKey` id `new_bytes` references (as a map
    /// key, recursively) into `tx.referenced_interner_ids` BEFORE calling
    /// this — e.g. `execute_update_tx`/`execute_set_tx` capture from the
    /// `set_map`/`new_inner` tree they built `new_bytes` from. Callers that
    /// build `new_bytes` some other way (FK cascade actions in
    /// `query/batch/fk_actions.rs` / `fk_on_update.rs`) pass `false`, which
    /// leaves this table on `pre_commit_prelock`'s A8 decode fallback —
    /// always correct, just not free. This is a PER-CALL flag, not
    /// per-table: this table's `StagingStore` tracks the ids-captured state
    /// across CALLS (a later `false` call downgrades it back), so mixed
    /// captured/uncaptured updates to the same table within one tx are
    /// still handled correctly regardless of call order.
    pub(crate) async fn update_tx_bytes(
        &self,
        id: RecordId,
        old_bytes: &Bytes,
        new_bytes: Bytes,
        tx: &mut shamir_tx::TxContext,
        ids_captured: bool,
    ) -> DbResult<()> {
        // Level-3: acquire an Exclusive lock on the key before staging.
        self.acquire_pessimistic_write_lock(RecordKey::from_slice(id.as_bytes()), tx)
            .await?;

        // P0-2 (#958) / TOCTOU fix (2b): capture generations BEFORE any
        // `all_backends()` / `iter()` snapshot (see `insert_tx`'s comment).
        let token = self.table_token();
        let index2_gen = self.index2_registry().generation();
        let sorted_gen = self.sorted_indexes.generation();
        let base_index_gen = self.index_manager.generation();

        // Build RecordView lenses for old and new. For non-map records
        // (bare scalars from legacy tests), fall back to a full InnerValue
        // decode so the planners still see the value.
        let tx_id = Some(tx.tx_id);
        let (index_ops, counter_delta) =
            match (RecordView::new(old_bytes), RecordView::new(&new_bytes)) {
                (Ok(old_view), Ok(new_view)) => {
                    // Stage-time unique validation via the lens.
                    //
                    // #1096 follow-up (found by `@oh` review): tx-aware —
                    // this is the wire path `execute_update_tx` calls for
                    // every transactional UPDATE, so it needs the same
                    // released-key + touched-record treatment `update_tx`
                    // has (see `validate_unique_for_create_with_released`'s doc
                    // for why both are required, not just the released-key set).
                    // The on-demand `is_record_touched` probe eliminates the O(N²)
                    // cost by avoiding an O(N) set build for every row.
                    //
                    // Gated behind `has_unique_indexes()` (2nd `@oh` review,
                    // dbc6299e follow-up): this fn is called PER ROW on the
                    // wire UPDATE path — computing `released` unconditionally
                    // makes every UPDATE on a table with NO unique indexes pay
                    // an O(|tx.index_write_set|) walk for nothing.
                    if self.index_manager.has_unique_indexes() {
                        refresh_released_unique_cache(tx, self.table_token());
                        let released = &tx.released_unique_cache[&self.table_token()].released;
                        let table_token = self.table_token();
                        let is_record_touched =
                            |rid: &shamir_types::types::record_id::RecordId| -> bool {
                                tx.write_set
                                    .get(&table_token)
                                    .is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
                            };
                        self.index_manager
                            .validate_unique_for_update_with_released(
                                &id,
                                &old_view,
                                &new_view,
                                released,
                                &is_record_touched,
                            )
                            .await?;
                    }

                    // UniqueGuards for commit-time re-validation.
                    for index_key in self.index_manager.unique_keys_for(&new_view) {
                        tx.record_unique_guard(shamir_tx::UniqueGuard {
                            table_token: self.table_token(),
                            index_key,
                            owner: id,
                        });
                    }

                    // Index2 backends (RecordRef-generic).
                    let mut ops = self
                        .plan_update_ops_ref(id, &old_view, &new_view, tx_id)
                        .await?;
                    // base_index + sorted posting ops.
                    ops.extend(
                        self.plan_base_index_update_ops_ref(id, &old_view, &new_view)
                            .await?,
                    );

                    // HNSW vector staging (RecordRef-generic via the backend trait).
                    let token = self.table_token();
                    for backend in self.index2_registry.all_backends().await {
                        if let Some(v) = backend.staged_vector(id, &new_view).await {
                            tx.stage_vector(token, id, v);
                        }
                    }

                    (ops, 0_i64)
                }
                _ => {
                    // Non-map fallback: decode to InnerValue trees.
                    let old_tree = InnerValue::from_bytes(old_bytes.clone()).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "update_tx_bytes: old decode: {e}"
                        ))
                    })?;
                    let new_tree = InnerValue::from_bytes(new_bytes.clone()).map_err(|e| {
                        shamir_storage::error::DbError::Codec(format!(
                            "update_tx_bytes: new decode: {e}"
                        ))
                    })?;

                    // Gated behind `has_unique_indexes()` for the same
                    // reason as the map-lens branch above — computing
                    // `released` unconditionally makes every UPDATE on a
                    // table with NO unique indexes pay an O(|tx.index_write_set|)
                    // walk for nothing. The on-demand `is_record_touched` probe
                    // eliminates the O(N²) cost by avoiding an O(N) set build
                    // for every row.
                    if self.index_manager.has_unique_indexes() {
                        refresh_released_unique_cache(tx, self.table_token());
                        let released = &tx.released_unique_cache[&self.table_token()].released;
                        let table_token = self.table_token();
                        let is_record_touched =
                            |rid: &shamir_types::types::record_id::RecordId| -> bool {
                                tx.write_set
                                    .get(&table_token)
                                    .is_some_and(|s| s.staged_op(rid.as_bytes()).is_some())
                            };
                        self.index_manager
                            .validate_unique_for_update_with_released(
                                &id,
                                &old_tree,
                                &new_tree,
                                released,
                                &is_record_touched,
                            )
                            .await?;
                    }
                    for index_key in self.index_manager.unique_keys_for(&new_tree) {
                        tx.record_unique_guard(shamir_tx::UniqueGuard {
                            table_token: self.table_token(),
                            index_key,
                            owner: id,
                        });
                    }

                    let mut ops = self
                        .plan_update_ops(id, &old_tree, &new_tree, tx_id)
                        .await?;
                    ops.extend(
                        self.plan_base_index_update_ops(id, &old_tree, &new_tree)
                            .await?,
                    );
                    self.stage_vectors(id, &new_tree, tx).await;

                    (ops, 0_i64)
                }
            };

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Set(RecordKey::from_slice(id.as_bytes()), new_bytes),
                index_ops,
                counter_delta,
            },
            tx,
        )
        .await?;
        // #1205: see this fn's doc for the `ids_captured` contract.
        if ids_captured {
            self.mark_staged_ids_captured(tx);
        }

        // F-50 (#869) + F-50 Step 2 (#870) + P0-2 (#958): capture the index2 /
        // sorted / base_index generation (values read ABOVE, before the stage-time
        // snapshots).
        tx.note_index2_stage_gen(token, index2_gen);
        tx.note_sorted_stage_gen(token, sorted_gen);
        tx.note_base_index_stage_gen(token, base_index_gen);

        Ok(())
    }
}
