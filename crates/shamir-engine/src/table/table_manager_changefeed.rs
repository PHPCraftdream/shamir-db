use shamir_collections::TFxMap;

use super::table_manager::TableManager;

impl TableManager {
    /// Record a non-tx write footprint in the SSI commit-write log so that
    /// Serializable transactions can detect phantom conflicts caused by
    /// non-transactional writes (MVCC-1 fix).
    ///
    /// Called by the non-tx `execute_insert/update/set/delete` methods AFTER
    /// the mutations are durable. No-op when no gate is wired (`changefeed ==
    /// None`) or when `commit_version == 0` (no MvccStore attached).
    ///
    /// `index_ops` is the list of `IndexWriteOp`s planned by `sorted_indexes`
    /// for this write — only `SetPosting` variants contribute to the
    /// `inserted_index_keys` of the footprint (mirroring `build_footprint_from_tx`).
    /// For deletes (no new postings), pass an empty slice; `touched = true`
    /// still marks the table as written, which is enough for coarse `TableScan`
    /// conflict detection.
    pub(crate) fn record_nontx_ssi_footprint(
        &self,
        commit_version: u64,
        index_ops: &[shamir_tx::IndexWriteOp],
    ) {
        // No gate wired → system table or test without SSI wiring.
        // commit_version == 0 → no MvccStore (pure in-memory test without MVCC).
        let Some(cf) = &self.changefeed else {
            return;
        };
        if commit_version == 0 {
            return;
        }

        // No Serializable tx watching → nothing can observe this footprint;
        // skip the record entirely (honours Snapshot/level-1 "no overhead").
        if cf.gate.active_serializable_count() == 0 {
            return;
        }

        let table_token = self.table_token();
        let mut footprint = shamir_tx::TableWriteFootprint {
            touched: true,
            inserted_index_keys: Vec::with_capacity(index_ops.len()),
        };
        for op in index_ops {
            if let shamir_tx::IndexWriteOp::SetPosting { key, .. } = op {
                footprint.inserted_index_keys.push(key.clone());
            }
        }
        footprint.inserted_index_keys.sort_unstable();

        let rec = shamir_tx::CommitWriteRecord {
            commit_version,
            per_table: TFxMap::from_iter([(table_token, footprint)]),
        };
        cf.gate.record_commit_writes(rec);
        // Advance last_committed so that new snapshots opened after this
        // non-tx write pick up the correct baseline, and Phase 2-bis window
        // `(snapshot, last_committed]` correctly includes this write.
        // Uses fetch_max semantics to avoid backward movement when racing
        // with concurrent tx commits or other non-tx writes.
        cf.gate.publish_committed_max(commit_version);
    }
}
