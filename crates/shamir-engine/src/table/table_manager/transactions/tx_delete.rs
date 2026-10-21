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

    /// tx-aware delete.
    ///
    /// `tx == None` → existing `delete`.
    /// `tx == Some` → reads old value as raw bytes, plans delete ops via a
    /// zero-copy `RecordView` lens (no `InnerValue` tree decode), stages
    /// Remove. Returns `true` if a record was present.
    ///
    /// HIGH-6 / gap#1: any vector the deleted row carries is routed
    /// tx-locally into `tx.staged_vector_deletes` (via
    /// `stage_vector_delete`, the delete
    /// mirror of the insert path's `stage_vectors`). The tx-aware
    /// `plan_delete_tx` is a no-op on the live graph for a tx, so a
    /// dropped/aborted tx leaves no ghost. At commit (Phase 5d) the staged
    /// delete is promoted into the graph (`adapter.delete`) AND the durable
    /// delta log (`DeltaOp::Delete`).
    pub async fn delete_tx(
        &self,
        id: RecordId,
        tx: Option<&mut shamir_tx::TxContext>,
    ) -> DbResult<bool> {
        let Some(tx) = tx else {
            return self.delete(id).await;
        };

        // Read the raw storage bytes — no InnerValue decode. `None` means
        // the record is absent (staged-removed or not found).
        let Some(old_bytes) = self.read_one_tx_bytes(id, Some(&*tx)).await? else {
            return Ok(false);
        };

        // Level-3: acquire an Exclusive lock on the key before staging the
        // delete. Re-entrant upgrade from the Shared lock `read_one_tx_bytes`
        // took. No-op for Snapshot / Serializable.
        self.acquire_pessimistic_write_lock(RecordKey::from_slice(id.as_bytes()), tx)
            .await?;

        // Plan index-removal ops. Try the zero-copy RecordView lens first
        // (the fast path for map records — the production shape). If the
        // record is a non-map scalar (e.g. a bare string stored by legacy
        // tests), RecordView::new fails; fall back to a full InnerValue
        // decode so the planners still see the value and produce correct
        // (typically empty) index ops.
        //
        // P0-2 (#958) / TOCTOU fix (2b): capture generations BEFORE any
        // `all_backends()` / `iter()` snapshot (see `insert_tx`'s comment).
        let token = self.table_token();
        let index2_gen = self.index2_registry().generation();
        let sorted_gen = self.sorted_indexes.generation();
        let base_index_gen = self.index_manager.generation();
        //
        // gap#1 / HIGH-6: in BOTH branches we also stage any vector
        // delete the record carries into `tx.staged_vector_deletes` — the
        // delete mirror of the insert path's `stage_vectors`. This is the
        // sole staging path for tx vector deletes; the tx-aware
        // `plan_delete_tx` is a no-op on the live graph for a tx (like
        // `plan_insert_tx`), so a dropped/aborted tx leaves no ghost.
        let tx_id = Some(tx.tx_id);
        let index_ops = match RecordView::new(&old_bytes) {
            Ok(old_view) => {
                let mut ops = self.plan_delete_ops(id, &old_view, tx_id).await?;
                ops.extend(self.plan_base_index_delete_ops(id, &old_view).await?);
                self.stage_vector_delete(id, &old_view, tx).await;
                ops
            }
            Err(_) => {
                // Non-map record — decode to InnerValue tree for the planners.
                let old_inner = InnerValue::from_bytes(old_bytes.clone()).map_err(|e| {
                    shamir_storage::error::DbError::Codec(format!(
                        "delete_tx: failed to decode record bytes for {:?}: {}",
                        id, e
                    ))
                })?;
                let mut ops = self.plan_delete_ops(id, &old_inner, tx_id).await?;
                ops.extend(self.plan_base_index_delete_ops(id, &old_inner).await?);
                self.stage_vector_delete(id, &old_inner, tx).await;
                ops
            }
        };

        self.stage_mutation(
            StagedMutation {
                data_op: KvOp::Remove(RecordKey::from_slice(id.as_bytes())),
                index_ops,
                counter_delta: -1,
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

        Ok(true)
    }

    /// tx-aware insert-or-update by RecordId. Alias of `update_tx`
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
