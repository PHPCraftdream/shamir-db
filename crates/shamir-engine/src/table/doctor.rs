//! The "doctor" — integrity verification and repair.
//!
//! `data_store` is the source of truth in ShamirDb; every index +
//! the record counter is derived state. If a crash or a buggy
//! upgrade leaves the derived state inconsistent, the doctor can
//! either *check* (`verify`) or *fix* (`repair`) it from data.
//!
//! - **`verify()`** — read-only audit. Scans data, counts expected
//!   index entries vs actual. Returns a `VerifyReport`. Never
//!   modifies anything.
//!
//! - **`repair()`** — drops every index, recounts the counter,
//!   recreates every index from scratch by replaying
//!   `on_record_created` for every record in the data store. Slow
//!   but always correct.
//!
//! - **`recover_on_open()`** — call once on database open. Reads
//!   the WAL; if anything is in flight, runs `repair()` and
//!   clears the markers. Cheap on clean shutdown (one prefix scan
//!   returning zero entries).

use std::time::Instant;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use shamir_storage::error::DbResult;

use crate::index::index_definition::IndexDefinition;
use crate::index::index_manager::IndexManager;
use crate::index::sorted_index_manager::{SortedIndexDefinition, SortedIndexManager};

use super::table_manager::TableManager;

/// Read-only audit report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub records_in_data: u64,
    pub counter_value: u64,
    pub counter_consistent: bool,
    pub regular_indexes: Vec<IndexHealth>,
    pub unique_indexes: Vec<IndexHealth>,
    pub sorted_indexes: Vec<IndexHealth>,
    pub elapsed_ms: u64,
}

impl VerifyReport {
    pub fn is_healthy(&self) -> bool {
        self.counter_consistent && self.all_indexes_healthy()
    }

    pub fn all_indexes_healthy(&self) -> bool {
        self.regular_indexes.iter().all(|i| i.is_healthy())
            && self.unique_indexes.iter().all(|i| i.is_healthy())
            && self.sorted_indexes.iter().all(|i| i.is_healthy())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexHealth {
    pub name_interned: u64,
    pub expected_entries: u64,
    pub actual_entries: u64,
}

impl IndexHealth {
    pub fn is_healthy(&self) -> bool {
        self.expected_entries == self.actual_entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairReport {
    pub records_scanned: u64,
    pub counter_before: u64,
    pub counter_after: u64,
    pub regular_indexes_rebuilt: u64,
    pub unique_indexes_rebuilt: u64,
    pub sorted_indexes_rebuilt: u64,
    pub elapsed_ms: u64,
}

impl TableManager {
    /// Run a read-only consistency audit. Never modifies state.
    pub async fn verify(&self) -> DbResult<VerifyReport> {
        let start = Instant::now();

        let counter_value = self.count().await? as u64;
        let regular_defs: Vec<IndexDefinition> = self.index_manager_ref().iter_indexes().collect();
        let unique_defs: Vec<IndexDefinition> =
            self.index_manager_ref().iter_unique_indexes().collect();
        let sorted_defs: Vec<SortedIndexDefinition> = self.sorted_indexes().iter_indexes();

        // Single streaming pass over the data store: count records
        // and tally "should have an entry" per index.
        let mut records_in_data: u64 = 0;
        let mut expected_regular = vec![0u64; regular_defs.len()];
        let mut expected_unique = vec![0u64; unique_defs.len()];
        let mut expected_sorted = vec![0u64; sorted_defs.len()];

        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (_id, value) in batch? {
                records_in_data += 1;
                for (i, def) in regular_defs.iter().enumerate() {
                    if IndexManager::extract_index_values(&value, &def.paths).is_some() {
                        expected_regular[i] += 1;
                    }
                }
                for (i, def) in unique_defs.iter().enumerate() {
                    if IndexManager::extract_index_values(&value, &def.paths).is_some() {
                        expected_unique[i] += 1;
                    }
                }
                for (i, def) in sorted_defs.iter().enumerate() {
                    if SortedIndexManager::has_indexable_value(&value, &def.field_path) {
                        expected_sorted[i] += 1;
                    }
                }
            }
        }

        // For each index pull the actual entry count from info_store.
        let mut regular_indexes = Vec::with_capacity(regular_defs.len());
        for (def, expected) in regular_defs.iter().zip(expected_regular.iter()) {
            let actual = self
                .index_manager_ref()
                .entry_count(def.name_interned, false)
                .await?;
            regular_indexes.push(IndexHealth {
                name_interned: def.name_interned,
                expected_entries: *expected,
                actual_entries: actual,
            });
        }
        let mut unique_indexes = Vec::with_capacity(unique_defs.len());
        for (def, expected) in unique_defs.iter().zip(expected_unique.iter()) {
            let actual = self
                .index_manager_ref()
                .entry_count(def.name_interned, true)
                .await?;
            unique_indexes.push(IndexHealth {
                name_interned: def.name_interned,
                expected_entries: *expected,
                actual_entries: actual,
            });
        }
        let mut sorted_indexes = Vec::with_capacity(sorted_defs.len());
        for (def, expected) in sorted_defs.iter().zip(expected_sorted.iter()) {
            let actual = self.sorted_indexes().entry_count(def.name_interned).await?;
            sorted_indexes.push(IndexHealth {
                name_interned: def.name_interned,
                expected_entries: *expected,
                actual_entries: actual,
            });
        }

        Ok(VerifyReport {
            records_in_data,
            counter_value,
            counter_consistent: counter_value == records_in_data,
            regular_indexes,
            unique_indexes,
            sorted_indexes,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Repair every derived state from `data_store`.
    ///
    /// Strategy:
    /// 1. Snapshot the index definitions (the *shapes*).
    /// 2. Drop every index — removes the on-disk posting entries.
    /// 3. Recreate each index — the existing create_index path
    ///    scans data and re-adds entries.
    /// 4. Recount the counter from the data store.
    pub async fn repair(&self) -> DbResult<RepairReport> {
        let start = Instant::now();

        let regular_defs: Vec<IndexDefinition> = self.index_manager_ref().iter_indexes().collect();
        let unique_defs: Vec<IndexDefinition> =
            self.index_manager_ref().iter_unique_indexes().collect();
        let sorted_defs: Vec<SortedIndexDefinition> = self.sorted_indexes().iter_indexes();

        let counter_before = self.count().await? as u64;

        for def in &regular_defs {
            let _ = self
                .index_manager_ref()
                .drop_index(def.name_interned)
                .await?;
        }
        for def in &unique_defs {
            let _ = self
                .index_manager_ref()
                .drop_unique_index(def.name_interned)
                .await?;
        }
        for def in &sorted_defs {
            let _ = self.sorted_indexes().drop_index(def.name_interned).await?;
        }

        // Recreate regular + unique via the existing create_index
        // path — it scans data + adds entries.
        for def in regular_defs.iter() {
            self.index_manager_ref().create_index(def.clone()).await?;
        }
        for def in unique_defs.iter() {
            self.index_manager_ref()
                .create_unique_index(def.clone())
                .await?;
        }
        // Sorted indexes don't have a create+backfill helper —
        // register the def, then replay on_record_created for
        // every record.
        for def in sorted_defs.iter() {
            self.sorted_indexes().register(def.clone()).await?;
        }
        if !sorted_defs.is_empty() {
            let stream = self.list_stream(1000);
            futures::pin_mut!(stream);
            while let Some(batch) = stream.next().await {
                let pairs = batch?;
                for (id, value) in &pairs {
                    self.sorted_indexes().on_record_created(id, value).await?;
                }
            }
        }

        // Recount counter from the data store.
        let mut counter_after: u64 = 0;
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            counter_after += batch?.len() as u64;
        }
        self.counter().set_to(counter_after).await?;

        Ok(RepairReport {
            records_scanned: counter_after,
            counter_before,
            counter_after,
            regular_indexes_rebuilt: regular_defs.len() as u64,
            unique_indexes_rebuilt: unique_defs.len() as u64,
            sorted_indexes_rebuilt: sorted_defs.len() as u64,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Crash-recovery entry point — call once on database open.
    ///
    /// Three-stage strategy ("roll forward by default, escalate if
    /// inconsistency persists"):
    ///
    /// 1. **Categorise WAL ops** into Created / Updated / Deleted
    ///    record_ids. Unknown / future variants → straight to
    ///    full repair.
    ///
    /// 2. **Targeted roll-forward**:
    ///    - **Created**: re-apply `on_records_created_batch` for
    ///      the records that DO exist in `data_store`. Idempotent
    ///      — adds missing posting entries; rewrites identical ones.
    ///    - **Updated**: same as Created (re-apply current value's
    ///      index entries). May leave OLD-value orphan entries —
    ///      caught by step 3.
    ///    - **Deleted**: for any record still present in data,
    ///      complete the delete (`self.delete(id)`). Removes both
    ///      data and index entries via the existing hook. If the
    ///      record is already absent, leaves any pre-existing
    ///      orphan index entries — caught by step 3.
    ///
    /// 3. **Verify + escalate**. After targeted roll-forward,
    ///    `verify()` reports any inconsistency. If unhealthy →
    ///    fall through to `repair()` (full rebuild). The user
    ///    sees a single `RepairReport` regardless of which path
    ///    ran.
    ///
    /// Cheap on clean shutdown: one prefix scan returning zero
    /// entries.
    pub async fn recover_on_open(&self) -> DbResult<Option<RepairReport>> {
        let inflight = self.wal().list_inflight().await?;
        if inflight.is_empty() {
            return Ok(None);
        }

        // 1. Categorise — collect record_ids per op kind. Unknown
        //    variants short-circuit to full repair.
        let mut created_ids: Vec<shamir_types::types::record_id::RecordId> = Vec::new();
        let mut updated_ids: Vec<shamir_types::types::record_id::RecordId> = Vec::new();
        let mut deleted_ids: Vec<shamir_types::types::record_id::RecordId> = Vec::new();
        let mut has_unknown_op = false;
        for entry in &inflight {
            match entry {
                shamir_wal::WalEntryAny::V1(v1) => {
                    for op in &v1.ops {
                        match op {
                            shamir_wal::WalOp::RecordCreated { record_id } => {
                                created_ids.push(*record_id);
                            }
                            shamir_wal::WalOp::RecordUpdated { record_id } => {
                                updated_ids.push(*record_id);
                            }
                            shamir_wal::WalOp::RecordDeleted { record_id } => {
                                deleted_ids.push(*record_id);
                            }
                            // Future variants (TxnBegin/Commit/Rollback,
                            // FtsTerm*, IndexCreated/Dropped, ...) — we
                            // don't know how to roll them forward, so we
                            // bail to full repair.
                            _ => has_unknown_op = true,
                        }
                    }
                }
                shamir_wal::WalEntryAny::V2(_) => {
                    // V2 entries appear only after stage 4 lands. For now,
                    // log and treat as unknown — recovery code for V2 is the
                    // RepoTxGate forward-fix path (not yet written).
                    log::warn!(
                        "WalEntryV2 found in recovery — V2 forward-fix not wired yet (stage 4)"
                    );
                    has_unknown_op = true;
                }
            }
        }
        if has_unknown_op {
            let report = self.repair().await?;
            for entry in &inflight {
                self.wal().commit(entry.txn_id()).await?;
            }
            return Ok(Some(report));
        }

        let start = Instant::now();

        // 2a. Created roll-forward — re-apply index hooks for
        //     records that exist in data.
        let mut records_processed: u64 = 0;
        if !created_ids.is_empty() {
            let values = self.table().get_many(&created_ids).await?;
            let pairs: Vec<(
                shamir_types::types::record_id::RecordId,
                shamir_types::types::value::InnerValue,
            )> = created_ids
                .iter()
                .zip(values.into_iter())
                .filter_map(|(id, opt)| opt.map(|v| (*id, v)))
                .collect();
            records_processed += pairs.len() as u64;
            let pairs_iter = || pairs.iter().map(|(id, v)| (id, v));
            self.index_manager_ref()
                .on_records_created_batch(pairs_iter())
                .await?;
            self.index_manager_ref()
                .on_records_created_unique_batch(pairs_iter())
                .await?;
            self.sorted_indexes()
                .on_records_created_batch(pairs_iter())
                .await?;
        }

        // 2b. Updated roll-forward — same shape as Created. Leaves
        //     OLD-value orphans if the indexed field changed; step
        //     3 catches them.
        if !updated_ids.is_empty() {
            let values = self.table().get_many(&updated_ids).await?;
            let pairs: Vec<(
                shamir_types::types::record_id::RecordId,
                shamir_types::types::value::InnerValue,
            )> = updated_ids
                .iter()
                .zip(values.into_iter())
                .filter_map(|(id, opt)| opt.map(|v| (*id, v)))
                .collect();
            records_processed += pairs.len() as u64;
            let pairs_iter = || pairs.iter().map(|(id, v)| (id, v));
            self.index_manager_ref()
                .on_records_created_batch(pairs_iter())
                .await?;
            self.index_manager_ref()
                .on_records_created_unique_batch(pairs_iter())
                .await?;
            self.sorted_indexes()
                .on_records_created_batch(pairs_iter())
                .await?;
        }

        // 2c. Deleted roll-forward — complete the delete for any
        //     record still in data_store. `self.delete` invokes
        //     on_record_deleted which removes both data and the
        //     index entries.
        for id in &deleted_ids {
            // Returns false if record was already absent; that's
            // fine — orphan postings (if any) are caught by step 3.
            // Ok-value (bool) intentionally discarded; ? propagates errors.
            let _ = self.delete(*id).await?;
        }
        records_processed += deleted_ids.len() as u64;

        // Reconcile counter from data_store. Required because the
        // WAL doesn't track exactly where the crash hit relative
        // to `counter.increment`.
        let mut count: u64 = 0;
        use futures::StreamExt;
        let stream = self.list_stream(1000);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            count += batch?.len() as u64;
        }
        self.counter().set_to(count).await?;

        // 3. Verify + escalate. `verify` is read-only, much cheaper
        //    than `repair` (no writes). If targeted roll-forward
        //    left orphans, escalate.
        let v = self.verify().await?;
        if !v.is_healthy() {
            log::warn!(
                "Targeted roll-forward left inconsistency in table '{}'; escalating to full repair. {:?}",
                self.name(),
                v,
            );
            let report = self.repair().await?;
            for entry in &inflight {
                self.wal().commit(entry.txn_id()).await?;
            }
            return Ok(Some(report));
        }

        // Targeted roll-forward succeeded. Clear markers.
        for entry in &inflight {
            self.wal().commit(entry.txn_id()).await?;
        }

        Ok(Some(RepairReport {
            records_scanned: records_processed,
            counter_before: 0,
            counter_after: count,
            regular_indexes_rebuilt: 0,
            unique_indexes_rebuilt: 0,
            sorted_indexes_rebuilt: 0,
            elapsed_ms: start.elapsed().as_millis() as u64,
        }))
    }
}
