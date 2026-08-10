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
use shamir_collections::TFxMap;
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::FULL_SCAN_BATCH;
use shamir_types::record_view::{RecordRef, RecordView};
use shamir_types::types::value::InnerValue;

use crate::index::index_definition::IndexDefinition;
use crate::index::sorted_index_manager::{SortedIndexDefinition, SortedIndexManager};
use crate::index2::state::IndexState;
use crate::table::record_cow::RecordCow;

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
    /// F-50 Step 3b — per-backend lifecycle health for index2 (`fts` /
    /// `functional` / `vector`). A `Building` entry is reported as
    /// `healthy == false` with a diagnostic `message`, and folds into
    /// [`is_healthy`](Self::is_healthy)'s AND-chain so a stuck-Building
    /// index surfaces in the overall report. (The self-healing open path
    /// normally clears these before any operator audit runs; this section
    /// is the belt-and-suspenders visibility layer for a Building index
    /// that survived open — e.g. a backfill that failed under the
    /// non-fatal `restore_on_open` error policy.)
    pub index2_backends: Vec<Index2Health>,
    /// R0-C (#1009): `by_id ↔ by_name ↔ persisted` consistency problems for
    /// the index2 registry, one human-readable diagnostic string per
    /// problem found. Empty when consistent (the common case — `insert()`'s
    /// #1009 check-before-mutate fix and `TableManager::create`'s open-path
    /// fail-closed behavior make a mismatch unreachable through normal use;
    /// this is defense-in-depth for any future code path that might
    /// reintroduce the hazard). Folds into
    /// [`is_healthy`](Self::is_healthy)'s AND-chain.
    pub index2_registry_consistency: Vec<String>,
    /// R0-C (#1010): index names that exist in MORE than one of the four
    /// index families (regular / unique / sorted / index2) on this table —
    /// a cross-family namespace collision. Each entry is a human-readable
    /// diagnostic naming the index and the families it was found in. Empty
    /// when no collision exists (the common case post-fix — CREATE now
    /// preflight-checks all four families under `ddl_admission`; this
    /// surfaces PRE-EXISTING collisions from before that fix landed, or any
    /// future regression). Deliberately NOT auto-repaired: there is no safe
    /// default choice of which family "wins" — an operator must resolve it
    /// explicitly (rename or drop one of the colliding indexes). Folds into
    /// [`is_healthy`](Self::is_healthy)'s AND-chain.
    pub cross_family_name_collisions: Vec<String>,
    pub elapsed_ms: u64,
}

impl VerifyReport {
    pub fn is_healthy(&self) -> bool {
        self.counter_consistent
            && self.all_indexes_healthy()
            && self.index2_registry_consistency.is_empty()
            && self.cross_family_name_collisions.is_empty()
    }

    pub fn all_indexes_healthy(&self) -> bool {
        self.regular_indexes.iter().all(|i| i.is_healthy())
            && self.unique_indexes.iter().all(|i| i.is_healthy())
            && self.sorted_indexes.iter().all(|i| i.is_healthy())
            && self.index2_backends.iter().all(|i| i.is_healthy())
    }
}

/// Per-index health snapshot for the regular / unique / sorted
/// (base_index) families.
///
/// `is_healthy()` requires BOTH (a) the entry-count check (`expected ==
/// actual`) AND (b) `state == IndexState::Ready`. A `Building` index
/// whose partial entry count HAPPENS to match (e.g. a fresh table with
/// zero rows, or a partial backfill that coincidentally matches) is still
/// reported unhealthy — it is permanently planner-invisible until an
/// explicit `doctor::repair()` rebuilds it (the base_index family has NO
/// automatic open-time self-heal, unlike index2's F-50 Step 3b). The
/// `message` field carries the operator-facing diagnostic for the
/// state-unhealthy case; it is `None` when `state == Ready`. Mirrors
/// [`Index2Health`]'s shape/behavior (P1-1, #966).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexHealth {
    pub name_interned: u64,
    pub expected_entries: u64,
    pub actual_entries: u64,
    pub state: IndexState,
    pub message: Option<String>,
}

impl IndexHealth {
    pub fn is_healthy(&self) -> bool {
        self.expected_entries == self.actual_entries && self.state == IndexState::Ready
    }
}

/// P1-1 (#966): operator-facing diagnostic for a base_index (regular /
/// unique / sorted) stuck in `Building` state. Mirrors `Index2Health`'s
/// message style (~line 224). `None` when `Ready` (the entry-count fields
/// are the diagnostic for a count mismatch; this message is specifically
/// for the state gap).
fn base_index_building_message(name_interned: u64, state: IndexState) -> Option<String> {
    (state != IndexState::Ready).then(|| {
        format!(
            "index name_interned={name_interned} is in Building state — backfill \
             was interrupted; run doctor::repair() to rebuild"
        )
    })
}

/// F-50 Step 3b — lifecycle health of a single index2 backend
/// (`fts` / `functional` / `vector`).
///
/// `healthy` is `false` iff `state == Building` (the build was interrupted
/// by a crash and the self-healing open path either has not run yet or its
/// backfill failed under the non-fatal error policy). The `message` field
/// carries the operator-facing diagnostic for unhealthy entries; it is
/// `None` for a `Ready` backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index2Health {
    pub id: u32,
    pub name: String,
    pub state: IndexState,
    pub healthy: bool,
    pub message: Option<String>,
}

impl Index2Health {
    pub fn is_healthy(&self) -> bool {
        self.healthy
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
                records_in_data += 1;
                // Closure that tallies index-leaf presence for a single record
                // via the `RecordRef` trait — works for both `RecordView` and
                // `InnerValue` impls.
                let mut tally = |rec: &(dyn RecordRef + '_)| {
                    for (i, def) in regular_defs.iter().enumerate() {
                        if crate::index::index_keys::extract_index_leaves(rec, &def.paths).is_some()
                        {
                            expected_regular[i] += 1;
                        }
                    }
                    for (i, def) in unique_defs.iter().enumerate() {
                        if crate::index::index_keys::extract_index_leaves(rec, &def.paths).is_some()
                        {
                            expected_unique[i] += 1;
                        }
                    }
                    for (i, def) in sorted_defs.iter().enumerate() {
                        if SortedIndexManager::has_indexable_value(rec, &def.field_path) {
                            expected_sorted[i] += 1;
                        }
                    }
                };
                match &cow {
                    RecordCow::Borrowed(bytes) => match RecordView::new(bytes) {
                        Ok(view) => tally(&view),
                        Err(_) => {
                            // Non-map record (bare scalar from legacy tests) —
                            // fall back to the full InnerValue decode, mirroring
                            // the write-exec delete/update paths.
                            let value = InnerValue::from_bytes(bytes.as_ref()).map_err(|e| {
                                shamir_storage::error::DbError::Codec(format!(
                                    "doctor verify: failed to decode record: {e}"
                                ))
                            })?;
                            tally(&value);
                        }
                    },
                    RecordCow::Owned(value) => tally(value),
                }
            }
        }

        // P0-3a (#1011): a `begin_write_barrier` acquisition was tried here
        // (mirroring `repair()`, F-3/#1030) to give the entry-count reads
        // below a stable DDL snapshot against a concurrent DROP INDEX.
        // REVERTED: `verify()` is a read-only DIAGNOSTIC whose whole purpose
        // includes inspecting a table with an index STUCK in `Building`
        // (e.g. a crashed/parked backfill) — exactly the scenario
        // `doctor_tests::verify_detects_building_regular_index`/
        // `_sorted_index` exercise by parking `create_index` mid-backfill
        // (which holds the SAME barrier) and then calling `verify()`
        // concurrently. Acquiring the barrier here self-deadlocks against
        // that in-flight CREATE — confirmed by a genuine 180s TIMEOUT on
        // both tests, not a flake. `verify()` must be able to report on a
        // table exactly while something else holds the barrier; only
        // `repair()` (a mutating self-heal, not read-only diagnosis) may
        // require exclusivity. The pre-existing transient-miscount race this
        // was meant to close remains open — same severity as before this
        // slice, not a new regression.
        //
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
                state: def.state,
                message: base_index_building_message(def.name_interned, def.state),
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
                state: def.state,
                message: base_index_building_message(def.name_interned, def.state),
            });
        }
        let mut sorted_indexes = Vec::with_capacity(sorted_defs.len());
        for (def, expected) in sorted_defs.iter().zip(expected_sorted.iter()) {
            // P0-3a (#1037): if the sorted index is being dropped, surface it as
            // a retryable error instead of silently reporting 0 entries (which would
            // be indistinguishable from a genuinely empty index). Mirrors slice 1's
            // behavior in table_manager_index_mgmt.rs (lines 1418-1424).
            let actual = match self.sorted_indexes().entry_count(def.name_interned).await {
                Ok(count) => count,
                Err(shamir_storage::error::DbError::IndexDrainInProgress(index_name)) => {
                    return Err(shamir_storage::error::DbError::NotFound(format!(
                        "sorted index '{index_name}' (interned={}) is currently being dropped; \
                         verify result unavailable — retry after drop completes",
                        def.name_interned
                    )));
                }
                Err(e) => return Err(e),
            };
            sorted_indexes.push(IndexHealth {
                name_interned: def.name_interned,
                expected_entries: *expected,
                actual_entries: actual,
                state: def.state,
                message: base_index_building_message(def.name_interned, def.state),
            });
        }

        // F-50 Step 3b — index2 backend lifecycle health. `all_descriptors()`
        // merges the authoritative tuple state into each cloned descriptor,
        // so `desc.state` here is the LIVE registry's truth (not the stale
        // serialization carrier on the backend's own `IndexDescriptor`).
        // A `Building` backend is reported unhealthy with the operator-facing
        // message from the Step 3a memo §4 design. R0-D (#1013): a `Failed`
        // backend is ALSO unhealthy — its message includes the underlying
        // recovery-failure reason (`IndexRegistry::failure_reason_of`), not
        // just the enum variant, so an operator sees WHY recovery failed,
        // not just THAT it did.
        let mut index2_backends = Vec::new();
        for desc in self.index2_registry().all_descriptors().await {
            let healthy = desc.state == IndexState::Ready;
            let message = match desc.state {
                IndexState::Ready => None,
                IndexState::Building => Some(format!(
                    "index2 backend '{}' (id={}) is in Building state — build was \
                     interrupted; reopen the table or run repair",
                    desc.name, desc.id
                )),
                IndexState::Failed => {
                    let reason = self
                        .index2_registry()
                        .failure_reason_of(desc.id)
                        .await
                        .unwrap_or_else(|| "no reason recorded".to_string());
                    Some(format!(
                        "index2 backend '{}' (id={}) is in Failed state — open-path recovery \
                         failed: {reason}",
                        desc.name, desc.id
                    ))
                }
            };
            index2_backends.push(Index2Health {
                id: desc.id,
                name: desc.name.clone(),
                state: desc.state,
                healthy,
                message,
            });
        }

        // R0-C (#1009): `by_id ↔ by_name ↔ persisted` consistency check for
        // the index2 registry. Cheap — cross-references the three
        // registry-level structures already loaded above / held in memory,
        // no additional store scan. See `VerifyReport::index2_registry_consistency`'s
        // doc for why this is defense-in-depth rather than a normally-reachable
        // condition post-#1009.
        let index2_registry_consistency = self.check_index2_registry_consistency().await?;

        // R0-C (#1010): cross-family index name collisions. Walks all four
        // families' names and reports any name present in more than one —
        // see `any_index_exists`'s doc for the CREATE-time preflight this
        // mirrors, and `VerifyReport::cross_family_name_collisions`'s doc for
        // why this is surfaced, not auto-repaired.
        let cross_family_name_collisions = self.check_cross_family_name_collisions().await;

        Ok(VerifyReport {
            records_in_data,
            counter_value,
            counter_consistent: counter_value == records_in_data,
            regular_indexes,
            unique_indexes,
            sorted_indexes,
            index2_backends,
            index2_registry_consistency,
            cross_family_name_collisions,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// R0-C (#1009): cross-reference `IndexRegistry`'s `by_id`/`by_name`
    /// maps against each other AND against the persisted `index2` metadata
    /// blob, without a full posting re-scan. Returns one diagnostic string
    /// per problem found (empty when consistent).
    ///
    /// Checks:
    ///   1. `by_id ↔ by_name`: every live descriptor (from `all_descriptors()`,
    ///      which reads the authoritative `by_id` tuple) must resolve back to
    ///      itself via `get_by_name(name_interned)` — i.e. `by_name` must
    ///      point at the SAME id `by_id` carries that name under. A mismatch
    ///      here means a backend is visible by id but unreachable (or
    ///      reachable under the wrong id) by name — exactly the #1009 defect
    ///      class this whole task closes at the source.
    ///   2. `live ↔ persisted`: every live descriptor's `(id, name_interned)`
    ///      must match what is currently durable on disk. A mismatch means
    ///      the in-memory registry has drifted from its own persisted
    ///      snapshot (e.g. a persist step was skipped after a mutation).
    async fn check_index2_registry_consistency(&self) -> DbResult<Vec<String>> {
        let mut problems = Vec::new();

        let live = self.index2_registry().all_descriptors().await;

        // (1) by_id -> by_name -> by_id round-trip.
        for desc in &live {
            match self.index2_registry().get_by_name(desc.name_interned).await {
                Some(backend) if backend.descriptor().id == desc.id => {}
                Some(backend) => {
                    problems.push(format!(
                        "index2 registry inconsistency: by_id entry {} ('{}') carries \
                         name_interned={}, but by_name resolves that name_interned to a \
                         DIFFERENT id ({})",
                        desc.id,
                        desc.name,
                        desc.name_interned,
                        backend.descriptor().id
                    ));
                }
                None => {
                    problems.push(format!(
                        "index2 registry inconsistency: by_id entry {} ('{}') carries \
                         name_interned={}, but by_name has no entry for it — the backend \
                         is visible by id but unreachable by name",
                        desc.id, desc.name, desc.name_interned
                    ));
                }
            }
        }

        // (2) live registry vs the persisted blob.
        if let Some(persisted) =
            crate::index2::persistence::load_index2_metadata(&self.info_store).await?
        {
            let persisted_by_id: TFxMap<u32, &crate::index2::IndexDescriptor> =
                persisted.descriptors.iter().map(|d| (d.id, d)).collect();
            for desc in &live {
                match persisted_by_id.get(&desc.id) {
                    Some(pd) if pd.name_interned == desc.name_interned => {}
                    Some(pd) => {
                        problems.push(format!(
                            "index2 registry inconsistency: live entry {} carries \
                             name_interned={} ('{}'), but the persisted descriptor for \
                             the same id carries name_interned={} ('{}') — registry has \
                             drifted from its own persisted snapshot",
                            desc.id, desc.name_interned, desc.name, pd.name_interned, pd.name
                        ));
                    }
                    None => {
                        problems.push(format!(
                            "index2 registry inconsistency: live entry {} ('{}') has no \
                             matching persisted descriptor — registered in memory but not \
                             durable",
                            desc.id, desc.name
                        ));
                    }
                }
            }
        } else if !live.is_empty() {
            problems.push(format!(
                "index2 registry inconsistency: {} live backend(s) registered but no \
                 persisted index2 metadata exists on disk",
                live.len()
            ));
        }

        Ok(problems)
    }

    /// R0-C (#1010): walk all four index families' names on this table and
    /// report any name present in more than one family. Returns one
    /// diagnostic string per colliding name (empty when no collision).
    ///
    /// This is a startup/doctor-time SURFACE for pre-existing collisions
    /// (tables that acquired a cross-family duplicate before the #1010
    /// CREATE-time preflight landed) — it does not repair anything, since
    /// there is no safe default choice of which family should keep the name.
    async fn check_cross_family_name_collisions(&self) -> Vec<String> {
        let mut names: TFxMap<u64, Vec<&'static str>> = shamir_collections::new_fx_map();
        for def in self.index_manager_ref().iter_indexes() {
            names.entry(def.name_interned).or_default().push("regular");
        }
        for def in self.index_manager_ref().iter_unique_indexes() {
            names.entry(def.name_interned).or_default().push("unique");
        }
        for def in self.sorted_indexes().iter_indexes() {
            names.entry(def.name_interned).or_default().push("sorted");
        }
        for desc in self.index2_registry().all_descriptors().await {
            names.entry(desc.name_interned).or_default().push("index2");
        }

        let interner = self.interner.get().await.ok();
        let mut problems: Vec<String> = names
            .into_iter()
            .filter(|(_, families)| families.len() > 1)
            .map(|(name_interned, families)| {
                let display_name = interner
                    .as_ref()
                    .and_then(|i| {
                        i.get_str(&shamir_types::core::interner::InternerKey::new(
                            name_interned,
                        ))
                    })
                    .map(|s| (*s).to_string())
                    .unwrap_or_else(|| format!("<name_interned={name_interned}>"));
                format!(
                    "cross-family name collision: '{display_name}' exists in more than one \
                     index family: {}. This is not auto-repaired — no safe default choice \
                     of which family should keep the name; rename or drop one of the \
                     colliding indexes explicitly.",
                    families.join(", ")
                )
            })
            .collect();
        problems.sort();
        problems
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

        // F-3 (#1030): hold `ddl_admission` (via `begin_write_barrier`) for
        // the WHOLE drop -> recreate self-heal sequence below. Pre-fix, these
        // direct manager-level primitives (`index_manager_ref().drop_index`/
        // `drop_unique_index`, `sorted_indexes().drop_index`,
        // `sorted_indexes().register`) bypassed the barrier the
        // `TableManager` wrapper methods (`drop_index`/`drop_unique_index`/
        // `drop_sorted_index`) take — violating the precondition
        // `IndexRegistry::insert`'s doc comment states every
        // registry-mutating DDL op must uphold (see
        // `crates/shamir-index/src/registry.rs`, "# Precondition"): a
        // concurrent `CREATE INDEX` could race this self-heal's direct
        // drops/registers to the same next generation-counter value,
        // un-serialized. One `begin_write_barrier` acquisition covering the
        // entire block (not one per call, which would only serialize against
        // itself) restores that guarantee. Bit choice: `REGULAR_INDEX_CREATE`
        // — the same single `ddl_admission` mutex backs every bit
        // (`begin_write_barrier`'s Step 0), so any one existing bit is
        // sufficient for the admission serialization this fix needs; `repair()`
        // is an existing, accepted maintenance operation, so no new bit is
        // minted. The raw manager primitives called below do NOT themselves
        // acquire `ddl_admission`/`unique_write_lock` (confirmed by reading
        // `TableManager::drop_index`/`drop_unique_index`/`drop_sorted_index`,
        // which call these exact primitives AFTER already taking the
        // barrier) — so acquiring it here once cannot re-enter the
        // non-reentrant `tokio::sync::Mutex`.
        let (_barrier, _uwl_guard) = self
            .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
            .await;

        for def in &regular_defs {
            let _ = self
                .index_manager_ref()
                .drop_index(def.name_interned, None, None)
                .await?;
        }
        for def in &unique_defs {
            let _ = self
                .index_manager_ref()
                .drop_unique_index(def.name_interned, None, None)
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
                for (id, cow) in batch? {
                    match &cow {
                        RecordCow::Borrowed(bytes) => match RecordView::new(bytes) {
                            Ok(view) => {
                                self.sorted_indexes()
                                    .on_record_created(&id, &view, 0)
                                    .await?;
                            }
                            Err(_) => {
                                // Non-map record — fall back to full decode.
                                let value =
                                    InnerValue::from_bytes(bytes.as_ref()).map_err(|e| {
                                        shamir_storage::error::DbError::Codec(format!(
                                            "doctor repair: failed to decode record: {e}"
                                        ))
                                    })?;
                                self.sorted_indexes()
                                    .on_record_created(&id, &value, 0)
                                    .await?;
                            }
                        },
                        RecordCow::Owned(value) => {
                            self.sorted_indexes()
                                .on_record_created(&id, value, 0)
                                .await?;
                        }
                    }
                }
            }
        }
        drop(_barrier);
        drop(_uwl_guard);

        // Recount counter from the data store.
        let mut counter_after: u64 = 0;
        let recount_stream = self.list_stream(FULL_SCAN_BATCH);
        futures::pin_mut!(recount_stream);
        while let Some(batch) = recount_stream.next().await {
            counter_after += batch?.len() as u64;
        }
        self.counter().set_to(counter_after).await?;

        // F-50 Step 3b — optionally heal any index2 backend stuck in
        // `Building` (or, R0-D / #1013, `Failed`) state. The self-healing
        // open path is the PRIMARY recovery for `Building` (it runs
        // automatically on every table open); this `repair()` branch is the
        // manual belt-and-suspenders trigger for an operator who notices a
        // stuck `Building` OR `Failed` index on a LIVE table (the latter
        // left behind by a genuinely failed `drop_all`/`restore_on_open` at
        // open time — R0-D fails those CLOSED into `Failed` rather than
        // silently retrying, so `repair()` is the intended manual recovery
        // path for them) and wants to force a re-build without a full table
        // reopen. It reuses the same restart-from-scratch logic as the open
        // path: drop the partial postings, re-run the full backfill, flip
        // Ready, and re-persist. Unlike the open-path self-heal, a failed
        // `drop_all` here is reported but the backfill still runs — this is
        // an EXPLICIT operator-triggered retry, not a silent automatic one;
        // if the backfill also fails, `backfill_index2_backend`'s `?`
        // propagates and the index stays `Failed` (unhealed) rather than
        // being counted below.
        let mut index2_healed: u64 = 0;
        let index2_descs = self.index2_registry().all_descriptors().await;
        for desc in &index2_descs {
            if !matches!(desc.state, IndexState::Building | IndexState::Failed) {
                continue;
            }
            if let Some(backend) = self.index2_registry().get_by_id(desc.id).await {
                log::warn!(
                    "repair: re-triggering restart-from-scratch for {:?} index2 \
                     backend '{}' (id={})",
                    desc.state,
                    desc.name,
                    desc.id
                );
                if let Err(e) = backend.drop_all().await {
                    log::warn!(
                        "repair: drop_all for {:?} index2 '{}' (id={}) failed: {} \
                         — continuing with backfill",
                        desc.state,
                        desc.name,
                        desc.id,
                        e
                    );
                }
                self.backfill_index2_backend(backend.as_ref()).await?;
                self.index2_registry()
                    .set_state(desc.id, IndexState::Ready)
                    .await;
                index2_healed += 1;
            }
        }
        if index2_healed > 0 {
            let _ = crate::index2::persistence::save_index2_metadata(
                &self.index2_registry,
                &self.info_store,
            )
            .await;
        }

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
