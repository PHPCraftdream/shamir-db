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

use std::time::Instant;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;

use crate::index::index_definition::IndexDefinition;
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

        let stream = self.list_stream(FULL_SCAN_BATCH);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (_id, cow) in batch? {
                let value = cow.into_inner()?;
                records_in_data += 1;
                for (i, def) in regular_defs.iter().enumerate() {
                    if crate::index::index_keys::extract_index_leaves(&value, &def.paths).is_some()
                    {
                        expected_regular[i] += 1;
                    }
                }
                for (i, def) in unique_defs.iter().enumerate() {
                    if crate::index::index_keys::extract_index_leaves(&value, &def.paths).is_some()
                    {
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

        // Use the shared seam helper: routes attached→log / unattached→data_store.
        let all_records: Vec<(
            shamir_types::types::record_id::RecordId,
            shamir_types::types::value::InnerValue,
        )> = self.collect_all_current_records().await?;
        for def in regular_defs.iter() {
            self.index_manager_ref()
                .create_index_from_records(def.clone(), all_records.clone())
                .await?;
        }
        for def in unique_defs.iter() {
            self.index_manager_ref()
                .create_unique_index_from_records(def.clone(), all_records.clone())
                .await?;
        }
        // Sorted indexes don't have a create+backfill helper —
        // register the def, then replay on_record_created for
        // every record.
        for def in sorted_defs.iter() {
            self.sorted_indexes().register(def.clone()).await?;
        }
        if !sorted_defs.is_empty() {
            let stream = self.list_stream(FULL_SCAN_BATCH);
            futures::pin_mut!(stream);
            while let Some(batch) = stream.next().await {
                let pairs: Vec<_> = batch?
                    .into_iter()
                    .map(|(id, cow)| cow.into_inner().map(|v| (id, v)))
                    .collect::<Result<_, _>>()?;
                for (id, value) in &pairs {
                    self.sorted_indexes()
                        .on_record_created(id, value, 0)
                        .await?;
                }
            }
        }

        // Recount counter from the data store.
        let mut counter_after: u64 = 0;
        let recount_stream = self.list_stream(FULL_SCAN_BATCH);
        futures::pin_mut!(recount_stream);
        while let Some(batch) = recount_stream.next().await {
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
}
