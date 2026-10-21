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

/// Bundled mutation effect for one record. Built by *_tx methods,
/// applied by `stage_mutation` to a TxContext.
pub(super) struct StagedMutation {
    pub(super) data_op: KvOp,
    pub(super) index_ops: Vec<shamir_tx::IndexWriteOp>,
    pub(super) counter_delta: i64,
}

// ── #1098 test-only pause seam ──────────────────────────────────────────
//
// Mirrors `pre_commit.rs`'s `TEST_POST_PRELOCK_PRE_MATERIALIZE_HOOK`
// (F-48b, #867): a `#[cfg(test)]` `OnceLock<Arc<Hook>>` global, zero cost
// when unset. Fires in `insert_tx` strictly AFTER the (fixed, #1098)
// generation-capture block and BEFORE the `has_unique_indexes()`-gated
// validation/guard-recording block — i.e. textually BETWEEN the two
// reads whose relative order #1098 fixed. A test can install the hook,
// spawn an `insert_tx` call, busy-poll `reached` (no sleeps, matching
// this codebase's established pattern for these seams), drive a real
// `CREATE UNIQUE INDEX` to completion while the tx is parked, then
// `resume` it — deterministically reproducing the exact interleaving
// window #1098 closes, without relying on timing luck.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PostGenCapturePreUniqueCheckHook {
    pub(crate) reached: std::sync::atomic::AtomicUsize,
    pub(crate) resume: tokio::sync::Notify,
    pub(crate) armed: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
pub(crate) static TEST_POST_GEN_CAPTURE_PRE_UNIQUE_CHECK_HOOK: std::sync::OnceLock<
    std::sync::Arc<PostGenCapturePreUniqueCheckHook>,
> = std::sync::OnceLock::new();

/// Parks on `TEST_POST_GEN_CAPTURE_PRE_UNIQUE_CHECK_HOOK` if a test
/// installed one; a true no-op otherwise. One-shot (CAS true→false) so
/// only the FIRST caller to reach this seam actually parks.
async fn fire_post_gen_capture_pre_unique_check_test_hook() {
    #[cfg(test)]
    if let Some(hook) = TEST_POST_GEN_CAPTURE_PRE_UNIQUE_CHECK_HOOK.get() {
        hook.reached
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let should_pause = hook
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok();
        if should_pause {
            hook.resume.notified().await;
        }
    }
}

/// #1096: refresh `tx.released_unique_cache` for `table_token` (incremental).
///
/// Mirrors `pre_commit.rs`'s Step 1 owner-aware `RemovePosting` handling — a
/// `RemovePosting` planned against a record that no longer holds the key
/// (per THIS walk's own `live` tracking so far) must not mark the key
/// released, or a later caller could wrongly treat a still-live key as free.
/// Without this, this function's doc claim of being "the exact same walk"
/// as `pre_commit.rs`'s Step 1 was false — the two copies had diverged.
///
/// #1108: incrementally maintained, NOT a full re-walk of
/// `tx.index_write_set` on every call. `tx.released_unique_cache` caches, per
/// `table_token`, the `live`/`released` state as of some prefix length
/// (`ReleasedUniqueCache::walked_len`) of `tx.index_write_set`. Since that Vec
/// is append-only during staging (nothing removes/reorders entries until
/// `pre_commit.rs` runs at commit time, strictly after every staging call
/// this function serves), each call only folds in the NEW suffix
/// `[walked_len..]` — exactly the same per-op match arms as before, just
/// applied once per op ever instead of once per op per call. A caller
/// sequence of N stage-time calls against a tx whose `index_write_set` ends
/// at length M therefore costs `O(M)` total, not `O(N * M)` — closing the
/// residual O(N^2) `#1099` left in place (see this crate's `#1108` prompt).
///
/// #1111: This function ONLY refreshes the cache; callers MUST then borrow
/// `&tx.released_unique_cache[&table_token].released` directly. This avoids
/// the O(N²) clone that a single combined `&mut -> TFxSet` pattern would force.
pub(crate) fn refresh_released_unique_cache(tx: &mut shamir_tx::TxContext, table_token: u64) {
    let total_len = tx.index_write_set.len();
    let cache = tx.released_unique_cache.entry(table_token).or_default();

    if cache.walked_len < total_len {
        for (tt, op) in &tx.index_write_set[cache.walked_len..total_len] {
            if *tt != table_token {
                continue;
            }
            match op {
                IndexWriteOp::SetPosting {
                    key,
                    value,
                    provenance,
                } if provenance.family == IndexFamily::Unique => {
                    let owner = RecordId::try_from_bytes(value).unwrap_or_default();
                    cache.live.insert(key.to_vec(), owner);
                    cache.released.remove(key.as_ref());
                }
                IndexWriteOp::RemovePosting {
                    key,
                    provenance,
                    owner,
                } if provenance.family == IndexFamily::Unique => {
                    let removing_owner = owner.map(RecordId);
                    let current_owner = cache.live.get(key.as_ref()).copied();
                    if current_owner.is_none()
                        || removing_owner.is_none()
                        || current_owner == removing_owner
                    {
                        cache.live.remove(key.as_ref());
                        cache.released.insert(key.to_vec());
                    }
                    // else: stale removal (owner mismatch against this walk's
                    // own live tracking) -- the key's real current owner
                    // within this tx is preserved, so it must not be marked
                    // released.
                }
                _ => {}
            }
        }
        cache.walked_len = total_len;
    }
    // No return: caller borrows &cache.released directly after refresh.
}

impl TableManager {
    /// #1205 (A8 amortization): mark this table's `StagingStore` as having
    /// every `Set` staged so far had its referenced `InternerKey` ids
    /// already captured into `tx.referenced_interner_ids`, so
    /// `pre_commit_prelock`'s A8 scan can skip its decode-based fallback
    /// for this table.
    ///
    /// Callers MUST have genuinely captured those ids (via
    /// `shamir_tx::collect_referenced_ids` on an in-memory tree, or by
    /// recording ids as an interning closure ran) immediately before
    /// calling this — see `StagingStore::mark_referenced_ids_captured`'s
    /// contract. A no-op if this table has no staging entry (nothing was
    /// staged, e.g. an empty batch).
    pub(super) fn mark_staged_ids_captured(&self, tx: &mut shamir_tx::TxContext) {
        if let Some(staging) = tx.write_set.get_mut(&self.table_token()) {
            staging.mark_referenced_ids_captured();
        }
    }

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
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            let mut ops = backend
                .plan_insert_tx(rid, rec, tx_id)
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            // R0-B (#1008): the backend only knows its OWN construction-time
            // descriptor snapshot — stamp the REAL live instance epoch from
            // the registry (see `index2_provenance`'s doc for why this
            // two-step stamping is necessary for index2 specifically).
            self.stamp_index2_provenance(&backend, &mut ops).await;
            all_ops.extend(ops);
        }
        Ok(all_ops)
    }

    /// R0-B (#1008): overwrite the placeholder `Provenance` every index2
    /// backend's `plan_*`/`plan_*_tx` stamps (see
    /// `shamir_index::write_ops::index2_provenance`'s doc) with the REAL
    /// live instance epoch (`IndexRegistry::instance_epoch_of`) for
    /// `backend`'s id. Shared by every index2 op-planning call site in this
    /// file (`plan_insert_ops`/`plan_update_ops`/`plan_update_ops_ref`/
    /// `plan_delete_ops`) so the commit-time reconcile
    /// (`shamir-engine::tx::pre_commit::rederive_index2_ops_post_stage`) can
    /// correctly match a staged op back to "is this still the same
    /// instance". A backend somehow missing from the registry (should be
    /// unreachable — the backend came FROM `all_backends()`) leaves the
    /// placeholder `instance_epoch: 0` untouched, which the reconcile
    /// correctly treats as "no live definition matches" (retract).
    pub(super) async fn stamp_index2_provenance(
        &self,
        backend: &std::sync::Arc<dyn shamir_index::backend::IndexBackend>,
        ops: &mut [shamir_tx::IndexWriteOp],
    ) {
        if ops.is_empty() {
            return;
        }
        let id = backend.descriptor().id;
        // F-6 (2026-08-06): `descriptor()` is a construction-time snapshot —
        // `IndexRegistry::rename_entry` updates the authoritative
        // `BackendEntry.name_interned` (registry.rs) but never mutates the
        // backend's own `descriptor()`. This is self-consistent ONLY because
        // `pre_commit.rs`'s `live_index2` set (built from `all_backends()`)
        // reads the SAME stale snapshot for its match key — do not "fix" one
        // side to the authoritative value without also fixing the other, or
        // every index2 op staged before a RENAME starts mismatching and gets
        // wrongly retracted.
        let name_interned = backend.descriptor().name_interned;
        if let Some(instance_epoch) = self.index2_registry.instance_epoch_of(id).await {
            let provenance = shamir_tx::Provenance {
                family: shamir_tx::IndexFamily::Index2,
                name_interned,
                instance_epoch,
            };
            for op in ops.iter_mut() {
                op.set_provenance(provenance);
            }
        }
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

    /// HIGH-6 / gap#1: route any HNSW vector delete carried by `rec` into
    /// the tx's own `staged_vector_deletes` buffer instead of the live
    /// graph. Mirrors `stage_vectors` for the delete side: each vector
    /// backend checks whether `rec` carries a vector at its field path
    /// (`IndexBackend::staged_vector`); if so, the rid lands under this
    /// table's token. Promoted into the graph (adapter tombstone) + the
    /// durable delta log (`DeltaOp::Delete`) atomically at commit
    /// (Phase 5d), discarded by RAII on abort. A record that carries no
    /// vector at a backend's field path returns `None` and contributes
    /// nothing — so a non-vector-backed delete is a no-op here, and the
    /// non-tx path (which forwards to `plan_delete`) is untouched.
    pub(super) async fn stage_vector_delete<R>(
        &self,
        rid: RecordId,
        rec: &R,
        tx: &mut shamir_tx::TxContext,
    ) where
        R: RecordRef + Sync,
    {
        let token = self.table_token();
        for backend in self.index2_registry.all_backends().await {
            if backend.staged_vector(rid, rec).await.is_some() {
                tx.stage_vector_delete(token, rid);
            }
        }
    }

    /// gap#1 (update branch): stage a vector DELETE for every vector backend
    /// whose embedding field the update REMOVES — `old` carried a vector at
    /// the backend's field path, `new` does not. Mirrors the non-tx
    /// `plan_update`'s `else`-branch tombstone (which the tx path otherwise
    /// loses): without this, a tx UPDATE that drops the embedding field
    /// leaves the old vector live in the graph (a ghost that also survives
    /// restart, since no `DeltaOp::Delete` is appended). A backend where
    /// `new` still carries a vector contributes nothing here — the staged
    /// insert (`stage_vectors`) replaces it at promote time.
    pub(super) async fn stage_vector_deletes_on_update<Old, New>(
        &self,
        rid: RecordId,
        old: &Old,
        new: &New,
        tx: &mut shamir_tx::TxContext,
    ) where
        Old: RecordRef + Sync,
        New: RecordRef + Sync,
    {
        let token = self.table_token();
        for backend in self.index2_registry.all_backends().await {
            if backend.staged_vector(rid, old).await.is_some()
                && backend.staged_vector(rid, new).await.is_none()
            {
                tx.stage_vector_delete(token, rid);
            }
        }
    }

    /// Collect index ops from all index2 backends for an update.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// See `plan_insert_ops` for the `tx_id` parameter.
    pub(super) async fn plan_update_ops(
        &self,
        rid: RecordId,
        old: &InnerValue,
        new: &InnerValue,
        tx_id: Option<shamir_tx::TxId>,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>> {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            let mut ops = backend
                .plan_update_tx(rid, old, new, tx_id)
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            self.stamp_index2_provenance(&backend, &mut ops).await;
            all_ops.extend(ops);
        }
        Ok(all_ops)
    }

    /// RecordRef-generic variant of `plan_update_ops`.
    ///
    /// Accepts any `RecordRef` (`InnerValue` or `RecordView`) so the update
    /// path can feed zero-copy lenses instead of decoded trees.
    pub(super) async fn plan_update_ops_ref<R>(
        &self,
        rid: RecordId,
        old: &R,
        new: &R,
        tx_id: Option<shamir_tx::TxId>,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>>
    where
        R: RecordRef + Sync,
    {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            let mut ops = backend
                .plan_update_tx(rid, old, new, tx_id)
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            self.stamp_index2_provenance(&backend, &mut ops).await;
            all_ops.extend(ops);
        }
        Ok(all_ops)
    }

    /// Collect index ops from all index2 backends for a delete.
    /// Does NOT apply — ops go into tx.index_write_set for deferred apply.
    ///
    /// See `plan_insert_ops` for the `tx_id` parameter.
    ///
    /// Accepts any `RecordRef` (`InnerValue` or `RecordView`) so the delete
    /// path can feed a zero-copy lens instead of a decoded tree.
    pub(super) async fn plan_delete_ops<R>(
        &self,
        rid: RecordId,
        rec: &R,
        tx_id: Option<shamir_tx::TxId>,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>>
    where
        R: RecordRef + Sync,
    {
        let mut all_ops = Vec::new();
        for backend in self.index2_registry.all_backends().await {
            let mut ops = backend
                .plan_delete_tx(rid, rec, tx_id)
                .await
                .map_err(|e| shamir_storage::error::DbError::Internal(e.to_string()))?;
            self.stamp_index2_provenance(&backend, &mut ops).await;
            all_ops.extend(ops);
        }
        Ok(all_ops)
    }

    /// HIGH-6: collect base_index `IndexManager` (regular + unique) and
    /// `SortedIndexManager` posting ops for a tx insert. These ops use
    /// the *exact* physical key layout the non-tx readers expect
    /// (`lookup_by_index` / `check_unique_constraint` / `lookup_range`),
    /// so applying them at commit time produces postings indistinguishable
    /// from the non-tx `on_record_created` path. The unique ops do NOT
    /// validate — validation runs separately at stage time in `insert_tx`.
    pub(super) async fn plan_base_index_insert_ops(
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

    /// HIGH-6: base_index + sorted posting ops for a tx update.
    pub(super) async fn plan_base_index_update_ops(
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

    /// RecordRef-generic variant of `plan_base_index_update_ops`.
    ///
    /// Accepts any `RecordRef` (`InnerValue` or `RecordView`) so the update
    /// path can feed zero-copy lenses instead of decoded trees.
    pub(super) async fn plan_base_index_update_ops_ref<R>(
        &self,
        rid: RecordId,
        old: &R,
        new: &R,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>>
    where
        R: RecordRef + Sync,
    {
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

    /// HIGH-6: base_index + sorted posting ops for a tx delete.
    ///
    /// Accepts any `RecordRef` (`InnerValue` or `RecordView`) so the delete
    /// path can feed a zero-copy lens instead of a decoded tree.
    pub(super) async fn plan_base_index_delete_ops<R>(
        &self,
        rid: RecordId,
        old: &R,
    ) -> DbResult<Vec<shamir_tx::IndexWriteOp>>
    where
        R: RecordRef + Sync,
    {
        let mut ops = self.index_manager.plan_record_deleted(&rid, old).await?;
        ops.extend(
            self.index_manager
                .plan_record_deleted_unique(&rid, old)
                .await?,
        );
        ops.extend(self.sorted_indexes.plan_record_deleted(&rid, old)?);
        Ok(ops)
    }

}
