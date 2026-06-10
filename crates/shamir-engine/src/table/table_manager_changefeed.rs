use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use super::table_manager::TableManager;

impl TableManager {
    /// Project + emit a non-transactional write footprint to the changefeed.
    ///
    /// Called by the non-tx `execute_insert/update/set/delete` methods AFTER
    /// the mutations are applied to the table. Best-effort and non-blocking:
    ///
    /// * No feed wired (`changefeed == None`) → no-op.
    /// * Empty `changes` → no-op.
    /// * Otherwise build the event via [`shamir_tx::nontx_event`] using
    ///   the caller-supplied `commit_version` (which is the MVCC version
    ///   the data was written at — see `*_returning_version` helpers),
    ///   and hand it to the feed's two non-blocking tracks (broadcast
    ///   `send` + journal `try_send`). Neither track waits nor errors
    ///   back to us.
    ///
    /// `commit_version` is the version already allocated by the data-
    /// write path (`MvccStore::set_versioned[_many]` /
    /// `delete_versioned`), so the event carries the EXACT same version
    /// the record landed at — no second `assign_next_version` bump.
    /// For batches with per-record versions the caller passes the MAX
    /// (= last) version, matching the commit-version-per-batch semantic
    /// the tx path uses.
    pub(crate) async fn emit_nontx_changefeed(
        &self,
        commit_version: u64,
        changes: Vec<shamir_tx::RecordChange>,
    ) {
        let Some(cf) = &self.changefeed else {
            return; // no feed wired (system table / test)
        };
        if changes.is_empty() {
            return; // empty footprint — nothing to emit
        }
        let event = shamir_tx::nontx_event(
            &cf.repo,
            commit_version,
            shamir_types::access::Actor::System,
            changes,
        );
        if let Some(event) = event {
            cf.feed.emit(event);
        }
    }

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
            inserted_index_keys: Vec::new(),
        };
        for op in index_ops {
            if let shamir_tx::IndexWriteOp::SetPosting { key, .. } = op {
                footprint.inserted_index_keys.push(key.clone());
            }
        }
        footprint.inserted_index_keys.sort_unstable();

        let rec = shamir_tx::CommitWriteRecord {
            commit_version,
            per_table: std::collections::HashMap::from([(table_token, footprint)]),
        };
        cf.gate.record_commit_writes(rec);
        // Advance last_committed so that new snapshots opened after this
        // non-tx write pick up the correct baseline, and Phase 2-bis window
        // `(snapshot, last_committed]` correctly includes this write.
        // Uses fetch_max semantics to avoid backward movement when racing
        // with concurrent tx commits or other non-tx writes.
        cf.gate.publish_committed_max(commit_version);
    }

    /// Build one [`RecordChange`](shamir_tx::RecordChange) for a non-tx
    /// `Put` (insert / update / set): the raw `RecordId` key bytes plus the
    /// serialized new record bytes — byte-identical to what the tx staging
    /// path carries for the same mutation. Serialization failure (which
    /// would also have failed the data write upstream) yields `None` so the
    /// change is simply omitted rather than poisoning the batch.
    pub(crate) fn put_change(
        &self,
        id: RecordId,
        value: &InnerValue,
    ) -> Option<shamir_tx::RecordChange> {
        let bytes = value.to_bytes().ok()?;
        Some(shamir_tx::RecordChange {
            table: self.name.clone(),
            key: id.to_bytes(),
            op: shamir_tx::ChangeOp::Put,
            value: Some(bytes),
        })
    }

    /// Build one [`RecordChange`](shamir_tx::RecordChange) for a non-tx
    /// `Delete`: the raw `RecordId` key bytes, no value (mirrors the tx
    /// `Remove` projection).
    pub(crate) fn delete_change(&self, id: RecordId) -> shamir_tx::RecordChange {
        shamir_tx::RecordChange {
            table: self.name.clone(),
            key: id.to_bytes(),
            op: shamir_tx::ChangeOp::Delete,
            value: None,
        }
    }
}
