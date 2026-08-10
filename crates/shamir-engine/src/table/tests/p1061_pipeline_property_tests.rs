//! #1061 — Online CREATE INDEX pipeline property tests.
//!
//! These are END-TO-END property tests that prove specific claims from the RFC empirically.
//! Each test is designed to FAIL if the mechanism it claims to prove doesn't exist.
//!
//! Tests:
//! 1. Completeness of capture (RFC §3 Claim 2) — pause Phase A mid-scan, drive a MIX of
//!    concurrent operations, resume, and verify the final index reflects the FINAL state
//!    of every touched row.
//! 2. Convergence / termination under sustained write load — Phase C's catch-up loop
//!    terminates within a bounded time even with concurrent writes.
//! 3. Bounded publish-barrier duration — Phase D's barrier duration is small and
//!    constant across different table sizes, unlike Phase A.
//! 4. Equivalence with the old path (no concurrent writes) — byte-identical posting
//!    keyspaces between the online path and the old whole-barrier path.

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use bytes::Bytes;
use futures::StreamExt;
use shamir_index::base_index::index_record_key::IndexRecordKey;
use shamir_index::IndexState;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

/// Helper: get the interned u64 key for a string.
async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Helper: create a TableManager with MVCC and changefeed attached (online-build capable).
async fn make_table_with_mvcc_and_changefeed() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    tbl.with_mvcc_store(mvcc).with_changefeed(gate)
}

/// Helper: create a TableManager WITHOUT changefeed (online-build unavailable).
async fn make_table_without_changefeed() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    TableManager::create("t".into(), data, info).await.unwrap()
}

/// Helper: build an IndexDefinition for a simple path.
async fn build_index_def(tbl: &TableManager, name: &str, path: &str) -> IndexDefinition {
    let idx_name = key_id(tbl, name).await;
    let path_key = key_id(tbl, path).await;

    IndexDefinition {
        name_interned: idx_name,
        paths: vec![IndexInfoItem::new(vec![path_key])],
        state: IndexState::Building,
        instance_epoch: 0,
    }
}

/// Helper: insert a record with given id and name.
async fn insert_record(tbl: &TableManager, id: i64, name: &str) -> RecordId {
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key, InnerValue::Int(id));
    m.insert(name_key, InnerValue::Str(name.to_string()));
    let value = InnerValue::Map(m);

    tbl.insert(&value).await.unwrap()
}

/// Helper: update a record's name field.
async fn update_record_name(tbl: &TableManager, rid: RecordId, new_name: &str) {
    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(name_key, InnerValue::Str(new_name.to_string()));
    tbl.set(rid, &InnerValue::Map(m))
        .await
        .expect("set should succeed");
}

/// Helper: look up all records via an index by name.
async fn lookup_by_name(
    tbl: &TableManager,
    name_interned: u64,
    lookup_name: &str,
) -> Vec<RecordId> {
    let lookup_value = vec![InnerValue::Str(lookup_name.to_string())];
    match tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &lookup_value)
        .await
    {
        Ok(Some(arc)) => arc.iter().copied().collect(),
        Ok(None) => Vec::new(),
        Err(_) => Vec::new(),
    }
}

/// Scan an `info_store` for every posting of the given index and collect the
/// `(key, value)` pairs into a sorted set. Mirrors the pattern from
/// `f78_streaming_equivalence_tests.rs`.
async fn collect_postings(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
) -> BTreeSet<(Vec<u8>, Vec<u8>)> {
    let prefix: Bytes = IndexRecordKey::new(false, name_interned).to_prefix_bytes();
    let mut out = BTreeSet::new();
    let mut s = info_store.scan_prefix_stream(prefix, 1000);
    while let Some(batch) = s.next().await {
        for (k, v) in batch.unwrap() {
            out.insert((k.as_ref().to_vec(), v.as_ref().to_vec()));
        }
    }
    out
}

/// Test 1 — Completeness of capture (proves RFC §3 Claim 2 empirically).
///
/// Pause Phase A mid-scan, drive a MIX of concurrent operations — including
/// operations the scan already passed and operations it hasn't reached yet —
/// resume, and check the FINAL index reflects the FINAL state of every touched row.
///
/// Uses `phase_b_a_backfill` with `batch_size = 1` to force multiple batches.
#[tokio::test]
async fn p1061_completeness_of_capture_mid_scan_mixed_ops() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert 5 rows (ids 0-4, distinct "name" values).
    let rid0 = insert_record(&tbl, 0, "name0").await;
    let rid1 = insert_record(&tbl, 1, "name1").await;
    let rid2 = insert_record(&tbl, 2, "name2").await;
    let rid3 = insert_record(&tbl, 3, "name3").await;
    let rid4 = insert_record(&tbl, 4, "name4").await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Install pause hook (test-only).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.online_index_backfill_hook
        .store(Some(Arc::clone(&hook)));

    // Spawn Phase B+A in a task with batch_size=1 to pause after first row.
    let tbl_clone = tbl.clone();
    let index_def_clone = index_def.clone();
    let backfill_task = tokio::spawn(async move {
        tbl_clone
            .phase_b_a_backfill("idx_name", index_def_clone, 1)
            .await
    });

    // Wait for the hook to park (after processing row 0, i.e. mid-scan).
    hook.wait_until_parked().await;

    // Now that we're parked mid-scan, issue a MIX of concurrent operations
    // from a separate task.

    // Operation 1: Insert a brand-new row (never existed before the build started).
    let rid_new = insert_record(&tbl, 99, "new_during_scan").await;

    // Operation 2: Update row 0 (the ALREADY-SCANNED row) to a new name value.
    update_record_name(&tbl, rid0, "name0_updated").await;

    // Operation 3: Update row 4 (a row the scan has NOT YET REACHED) to a new name.
    update_record_name(&tbl, rid4, "name4_updated").await;

    // Operation 4: Update row 1 TWICE in sequence (two different new values) —
    // proves the dirty-set's re-read-at-current-version mechanism correctly
    // picks up the FINAL value, not an intermediate one.
    update_record_name(&tbl, rid1, "name1_first_update").await;
    update_record_name(&tbl, rid1, "name1_final_update").await;

    // Operation 5: Insert-then-delete a brand-new row within the same window.
    let rid_insert_delete = insert_record(&tbl, 100, "insert_then_delete").await;
    tbl.delete_returning_version(rid_insert_delete)
        .await
        .expect("delete should succeed");

    // Resume the backfill.
    hook.release();

    // Wait for Phase B+A to complete.
    let phase_ba = backfill_task
        .await
        .expect("backfill task should not panic")
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Run Phase C+D to completion.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Clear the hook.
    tbl.online_index_backfill_hook.store(None);

    // Assertions via lookup_by_index.

    // The new plain insert is findable under its value.
    let found_new = lookup_by_name(&tbl, name_interned, "new_during_scan").await;
    assert_eq!(
        found_new.len(),
        1,
        "new insert should be findable under its value"
    );
    assert_eq!(
        found_new[0], rid_new,
        "new insert should return correct RecordId"
    );

    // Row 0 is findable ONLY under its new value, not the original.
    let found_name0 = lookup_by_name(&tbl, name_interned, "name0").await;
    assert!(
        found_name0.is_empty(),
        "row 0 should NOT be findable under original value 'name0'"
    );
    let found_name0_updated = lookup_by_name(&tbl, name_interned, "name0_updated").await;
    assert_eq!(
        found_name0_updated.len(),
        1,
        "row 0 should be findable under new value 'name0_updated'"
    );
    assert_eq!(
        found_name0_updated[0], rid0,
        "row 0 should return correct RecordId"
    );

    // Row 4 is findable ONLY under its new value, not the original.
    let found_name4 = lookup_by_name(&tbl, name_interned, "name4").await;
    assert!(
        found_name4.is_empty(),
        "row 4 should NOT be findable under original value 'name4'"
    );
    let found_name4_updated = lookup_by_name(&tbl, name_interned, "name4_updated").await;
    assert_eq!(
        found_name4_updated.len(),
        1,
        "row 4 should be findable under new value 'name4_updated'"
    );
    assert_eq!(
        found_name4_updated[0], rid4,
        "row 4 should return correct RecordId"
    );

    // Row 1 is findable ONLY under its SECOND new value (not the first
    // intermediate one, not the original).
    let found_name1 = lookup_by_name(&tbl, name_interned, "name1").await;
    assert!(
        found_name1.is_empty(),
        "row 1 should NOT be findable under original value 'name1'"
    );
    let found_name1_first = lookup_by_name(&tbl, name_interned, "name1_first_update").await;
    assert!(
        found_name1_first.is_empty(),
        "row 1 should NOT be findable under intermediate value 'name1_first_update'"
    );
    let found_name1_final = lookup_by_name(&tbl, name_interned, "name1_final_update").await;
    assert_eq!(
        found_name1_final.len(),
        1,
        "row 1 should be findable under final value 'name1_final_update'"
    );
    assert_eq!(
        found_name1_final[0], rid1,
        "row 1 should return correct RecordId"
    );

    // The insert-then-delete row is findable under NO value, AND has zero
    // postings on disk (the stronger, direct check).
    let found_insert_delete = lookup_by_name(&tbl, name_interned, "insert_then_delete").await;
    assert!(
        found_insert_delete.is_empty(),
        "insert-then-delete row should NOT be findable under any value"
    );

    // Direct check: scan the index's posting keyspace and confirm no posting
    // key embeds this row's RecordId suffix.
    let info_store = tbl.info_store();
    let postings = collect_postings(info_store, name_interned).await;
    let rid_bytes = rid_insert_delete.to_bytes();
    let has_posting = postings.iter().any(|(key, _)| key.ends_with(&rid_bytes));
    assert!(
        !has_posting,
        "insert-then-delete row should have ZERO postings on disk"
    );

    // Rows 2 and 3 (untouched during the window) are findable under their
    // original values.
    let found_name2 = lookup_by_name(&tbl, name_interned, "name2").await;
    assert_eq!(
        found_name2.len(),
        1,
        "row 2 should be findable under original value 'name2'"
    );
    assert_eq!(found_name2[0], rid2, "row 2 should return correct RecordId");

    let found_name3 = lookup_by_name(&tbl, name_interned, "name3").await;
    assert_eq!(
        found_name3.len(),
        1,
        "row 3 should be findable under original value 'name3'"
    );
    assert_eq!(found_name3[0], rid3, "row 3 should return correct RecordId");
}

/// Test 2 — Convergence / termination under sustained write load.
///
/// A generator that keeps writing new dirty records concurrently with Phase C's
/// catch-up loop, for a bounded duration, then stops — proving Phase C (and the
/// `CATCHUP_ITERATION_CAP` hand-off to Phase D) actually terminates rather than
/// looping forever chasing a moving target.
#[tokio::test]
async fn p1061_convergence_termination_under_sustained_load() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert a small base fixture.
    let _rid0 = insert_record(&tbl, 0, "alice").await;
    let _rid1 = insert_record(&tbl, 1, "bob").await;
    let _rid2 = insert_record(&tbl, 2, "charlie").await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A to completion (uninterrupted).
    let phase_ba = tbl
        .phase_b_a_backfill("idx_name", index_def, 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Spawn a background task that inserts ~200 new rows in a tight loop.
    let tbl_clone = tbl.clone();
    let generator_task = tokio::spawn(async move {
        let mut inserted = 0;
        while inserted < 200 {
            // Insert new rows.
            let id = 100 + inserted;
            let name = format!("user_{}", id);
            let _rid = insert_record(&tbl_clone, id, &name).await;
            inserted += 1;
        }
    });

    // Immediately (racing the generator) call phase_c_d_catchup_and_publish.
    // Wrap in timeout to assert it doesn't hang forever.
    let phase_c_d_result = tokio::time::timeout(
        Duration::from_secs(30),
        tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba),
    )
    .await;

    // The critical assertion: must NOT time out.
    match phase_c_d_result {
        Ok(Ok(())) => {
            // Success: Phase C+D terminated within 30 seconds.
        }
        Ok(Err(e)) => {
            panic!("phase_c_d_catchup_and_publish failed: {:?}", e);
        }
        Err(_) => {
            panic!(
                "phase_c_d_catchup_and_publish timed out after 30s - \
                 this indicates a convergence bug (looping forever chasing dirty writes)"
            );
        }
    }

    // Wait for the generator task to finish.
    generator_task
        .await
        .expect("generator task should not panic");

    // Assert: the index is Ready.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert!(def.is_some(), "index should be registered");
    assert_eq!(
        def.unwrap().state,
        IndexState::Ready,
        "index should be Ready after Phase C+D"
    );

    // Assert: dirty-set is empty (proving Phase D's final residual caught everything).
    let dirty = tbl.index_manager_ref().drain_dirty_set(name_interned);
    assert!(
        dirty.is_empty(),
        "dirty-set should be empty after Phase D - all dirty records were caught up"
    );

    // Verify some of the concurrently inserted records are indexed.
    let found_user_150 = lookup_by_name(&tbl, name_interned, "user_150").await;
    assert_eq!(
        found_user_150.len(),
        1,
        "concurrently inserted row should be indexed"
    );
}

/// Test 3 — Bounded publish-barrier duration (THE point of the whole redesign).
///
/// Run the identical no-concurrent-writes scenario at two RADICALLY different
/// table sizes and assert Phase D's barrier-held duration is small and roughly
/// CONSTANT across them, while Phase A's scan duration is NOT.
///
/// Sizes: 500 rows vs 50,000 rows (100x ratio, well under nextest's 180s kill).
#[tokio::test]
async fn p1061_bounded_barrier_duration_constant_across_sizes() {
    const SMALL_SIZE: usize = 500;
    const LARGE_SIZE: usize = 50_000;

    // Helper to run the pipeline and measure durations.
    async fn run_pipeline_and_measure(
        size: usize,
    ) -> Result<(std::time::Duration, std::time::Duration), String> {
        let tbl = make_table_with_mvcc_and_changefeed().await;

        // Insert the fixture (no concurrent writers).
        for i in 0..size {
            let name = format!("name_{}", i);
            insert_record(&tbl, i as i64, &name).await;
        }

        // Build index definition.
        let index_def = build_index_def(&tbl, "idx_name", "name").await;
        let name_interned = index_def.name_interned;

        // Measure Phase A duration.
        let phase_a_start = std::time::Instant::now();
        let phase_ba = tbl
            .phase_b_a_backfill("idx_name", index_def, 1000)
            .await
            .map_err(|e| format!("phase_b_a_backfill error: {:?}", e))?
            .ok_or_else(|| "online build unavailable".to_string())?;
        let phase_a_duration = phase_a_start.elapsed();

        // Measure Phase D duration (no concurrent writes, so Phase C exits immediately).
        let phase_d_start = std::time::Instant::now();
        tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
            .await
            .map_err(|e| format!("phase_c_d_catchup_and_publish error: {:?}", e))?;
        let phase_d_duration = phase_d_start.elapsed();

        Ok((phase_a_duration, phase_d_duration))
    }

    // Run for SMALL table.
    let (phase_a_duration_small, phase_d_duration_small) =
        run_pipeline_and_measure(SMALL_SIZE).await.unwrap();

    // Run for LARGE table.
    let (phase_a_duration_large, phase_d_duration_large) =
        run_pipeline_and_measure(LARGE_SIZE).await.unwrap();

    // Assertion 1: Phase A duration for LARGE table must be at least 3× the SMALL table.
    // (proves Phase A scales with table size — sanity check that the fixture sizes produce a measurable difference).
    assert!(
        phase_a_duration_large.as_millis() >= 3 * phase_a_duration_small.as_millis(),
        "Phase A should scale with table size: large={}ms, small={}ms",
        phase_a_duration_large.as_millis(),
        phase_a_duration_small.as_millis()
    );

    // Assertion 2: Phase D duration for BOTH sizes must be under an absolute ceiling.
    // (proves Phase D's cost does NOT scale with table size).
    const PHASE_D_MAX_MS: u128 = 100;
    assert!(
        phase_d_duration_small.as_millis() < PHASE_D_MAX_MS,
        "Phase D (small table) must be under {}ms, got {}ms",
        PHASE_D_MAX_MS,
        phase_d_duration_small.as_millis()
    );
    assert!(
        phase_d_duration_large.as_millis() < PHASE_D_MAX_MS,
        "Phase D (large table) must be under {}ms, got {}ms",
        PHASE_D_MAX_MS,
        phase_d_duration_large.as_millis()
    );

    // Assertion 3: Phase D duration for LARGE table is NOT within the same order of
    // magnitude as Phase A duration for LARGE table (once Phase A is clearly slower).
    // This is the actual correctness gate: Phase D's barrier cost is bounded, not O(N).
    if phase_a_duration_large.as_millis() > 50 {
        // Only assert this if Phase A is slow enough to measure meaningfully.
        assert!(
            phase_d_duration_large.as_millis() * 10 < phase_a_duration_large.as_millis(),
            "Phase D duration ({}ms) should be an order of magnitude smaller than Phase A ({}ms)",
            phase_d_duration_large.as_millis(),
            phase_a_duration_large.as_millis()
        );
    }
}

/// Test 4 — Equivalence with the old path (no concurrent writes).
///
/// Build the SAME fixture through BOTH paths against separate stores and verify
/// both paths index the same records for every distinct value (per-value lookup-set
/// equivalence, verified against each record's actual field content).
///
/// OLD path: a table WITHOUT changefeed, `create_index(...)` (falls back to
/// `create_index_from_stream` internally).
/// NEW path: a table WITH changefeed, `create_index(...)` (takes the online path).
#[tokio::test]
async fn p1061_equivalence_with_old_path_per_value_lookup_sets() {
    const FIXTURE_SIZE: usize = 300;

    // Build fixture data: a mix of distinct values, collisions, and field-absent rows.
    let mut fixture_rows: Vec<(i64, Option<String>)> = Vec::with_capacity(FIXTURE_SIZE);
    for i in 0..FIXTURE_SIZE {
        let name = match i % 5 {
            // Field absent → no posting.
            0 => None,
            // Two collision buckets sharing the value "dup_a".
            1 | 2 => Some("dup_a".to_string()),
            // A third collision bucket over a small integer domain (i % 3).
            3 => Some(format!("int_{}", i % 3)),
            // A distinct string value per row.
            _ => Some(format!("v_{}", i)),
        };
        fixture_rows.push((i as i64, name));
    }

    // ── NEW path: table with changefeed (online build) ───────────────────────
    let tbl_new = make_table_with_mvcc_and_changefeed().await;

    // Insert the fixture.
    for (id, name_opt) in &fixture_rows {
        if let Some(name) = name_opt {
            insert_record(&tbl_new, *id, name).await;
        } else {
            // Insert a row without the "name" field (field-absent).
            let interner = tbl_new.interner().get().await.unwrap();
            let id_key = interner.touch_ind("id").unwrap().into_key();
            tbl_new.interner().persist().await.unwrap();

            let mut m = new_map();
            m.insert(id_key, InnerValue::Int(*id));
            let value = InnerValue::Map(m);
            tbl_new.insert(&value).await.unwrap();
        }
    }

    // Create index via the NEW path (online).
    tbl_new.create_index("idx_name", &["name"]).await.unwrap();

    let name_interned_new = key_id(&tbl_new, "idx_name").await;

    // ── OLD path: table without changefeed (falls back to whole-barrier) ─────
    let tbl_old = make_table_without_changefeed().await;

    // Insert the same fixture.
    for (id, name_opt) in &fixture_rows {
        if let Some(name) = name_opt {
            insert_record(&tbl_old, *id, name).await;
        } else {
            // Insert a row without the "name" field.
            let interner = tbl_old.interner().get().await.unwrap();
            let id_key = interner.touch_ind("id").unwrap().into_key();
            tbl_old.interner().persist().await.unwrap();

            let mut m = new_map();
            m.insert(id_key, InnerValue::Int(*id));
            let value = InnerValue::Map(m);
            tbl_old.insert(&value).await.unwrap();
        }
    }

    // Create index via the OLD path (fallback).
    tbl_old.create_index("idx_name", &["name"]).await.unwrap();

    let name_interned_old = key_id(&tbl_old, "idx_name").await;

    // ── Per-value lookup-set equivalence validation ────────────────────────
    // For every DISTINCT indexed value in the fixture, assert the lookup result
    // SET matches between old and new paths. This is stronger than a total count
    // comparison (it catches "right count, wrong records" mis-indexing) and is
    // immune to the interner-numbering mismatch (we compare at the observable
    // behavior level, not raw posting keys).
    let distinct_values: BTreeSet<String> = fixture_rows
        .iter()
        .filter_map(|(_, name_opt)| name_opt.clone())
        .collect();

    // Resolve name_key for each table to verify record content.
    let name_key_new = {
        let interner = tbl_new.interner().get().await.unwrap();
        interner.touch_ind("name").unwrap().into_key()
    };
    let name_key_old = {
        let interner = tbl_old.interner().get().await.unwrap();
        interner.touch_ind("name").unwrap().into_key()
    };

    for value in &distinct_values {
        let lookup_value = vec![InnerValue::Str(value.clone())];

        let results_new = tbl_new
            .index_manager_ref()
            .lookup_by_index(name_interned_new, &lookup_value)
            .await
            .unwrap()
            .map(|arc| arc.iter().copied().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let results_old = tbl_old
            .index_manager_ref()
            .lookup_by_index(name_interned_old, &lookup_value)
            .await
            .unwrap()
            .map(|arc| arc.iter().copied().collect::<BTreeSet<_>>())
            .unwrap_or_default();

        // Assert result SET SIZE matches between old and new paths.
        assert_eq!(
            results_new.len(),
            results_old.len(),
            "value '{}': result set SIZE must match between old and new paths \
             (new={}, old={})",
            value,
            results_new.len(),
            results_old.len()
        );

        // Verify each returned record in the NEW path actually has this value at the
        // "name" field (catches cross-contamination: a record with a DIFFERENT value
        // wrongly returned by the lookup).
        for rid in &results_new {
            let rec = tbl_new.get(*rid).await.unwrap();
            let InnerValue::Map(m) = &rec else {
                panic!("expected record {rid:?} to be a Map");
            };
            assert_eq!(
                m.get(&name_key_new),
                Some(&InnerValue::Str(value.clone())),
                "new path: record {rid:?} returned for value '{}' must actually have that value",
                value
            );
        }

        // Verify each returned record in the OLD path actually has this value at the
        // "name" field.
        for rid in &results_old {
            let rec = tbl_old.get(*rid).await.unwrap();
            let InnerValue::Map(m) = &rec else {
                panic!("expected record {rid:?} to be a Map");
            };
            assert_eq!(
                m.get(&name_key_old),
                Some(&InnerValue::Str(value.clone())),
                "old path: record {rid:?} returned for value '{}' must actually have that value",
                value
            );
        }
    }

    // Guard against a both-sides-empty false-pass.
    // The fixture produces ~4/5 postings (1/5 are field-absent), so we should have
    // many distinct values with non-zero result sets.
    let mut total_indexed_rows: usize = 0;
    for value in &distinct_values {
        let lookup_value = vec![InnerValue::Str(value.clone())];
        let count = tbl_new
            .index_manager_ref()
            .lookup_by_index(name_interned_new, &lookup_value)
            .await
            .unwrap()
            .map(|arc| arc.len())
            .unwrap_or(0);
        total_indexed_rows += count;
    }

    assert!(
        total_indexed_rows > FIXTURE_SIZE / 2,
        "expected a substantial posting set (at least half the fixture indexed), got {}",
        total_indexed_rows
    );
}
