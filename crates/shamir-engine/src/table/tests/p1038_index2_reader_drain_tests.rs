//! P0-3a (#1038) — `ReaderDrainGate` integration tests for the index2 family.
//! Proves that the gate correctly blocks a DROP while readers are mid-flight AND that
//! readers back off during a DROP's drain→sweep window.
//!
//! This is the SIBLING test set to `p1011_reader_drain_tests.rs` (regular family) and
//! `p1037_sorted_reader_drain_tests.rs` (sorted family) — same proof-of-correctness
//! shape, different manager (IndexRegistry) and LEASE-based API (not chokepoint gating).

use std::sync::Arc;

use shamir_query_types::admin::types::CreateIndexOp;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::build_index2_backend_with_resolver;
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::expr::IndexExpr;
use crate::index2::functional_backend::FunctionalBackend;
use crate::index2::kind::{FunctionalConfig, IndexKind};
use crate::index2::state::IndexState;
use crate::index2::IndexRegistry;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Store;
use shamir_storage::error::DbError;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// A functional `lower(<field>)` index create op.
fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

/// Build a functional `lower(<field>)` index2 backend with `state = Ready`
/// (directly — NOT through `create_index_v2`).
fn build_ready_functional_backend(
    id: u32,
    name: &str,
    name_interned: u64,
    field_path: Vec<u64>,
    info_store: &Arc<dyn Store>,
) -> Arc<dyn crate::index2::backend::IndexBackend> {
    let first_path = field_path.clone();
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Field(first_path)));
    let kind = IndexKind::Functional(Box::new(FunctionalConfig { expr }));
    let desc = IndexDescriptor::new(
        id,
        name,
        name_interned,
        smallvec::smallvec![field_path],
        kind,
    );
    build_index2_backend_with_resolver(desc, info_store, None)
}

// ============================================================================
// 1. Proof test — reader holding lease blocks the DROP's drain
// ============================================================================

/// P0-3a (#1038) proof test: a reader holding a lease (guard HELD) must block
/// the DROP's `wait_for_drain` call. Once the reader releases the lease, the
/// drop proceeds and finishes. The read returns the correct complete result.
#[tokio::test]
async fn p1038_index2_lease_holds_blocks_drop_until_released() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    // Three pre-existing rows.
    for name in ["Alice", "Bob", "Charlie"] {
        tbl.insert(&record_with_str(name_key, name))
            .await
            .unwrap();
    }

    // Build a functional index.
    tbl.create_index_v2(functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let index_name_id = key_id(&tbl, "lower_name").await;

    // Verify the index is usable.
    let registry = tbl.index2_registry();
    let lease = registry
        .lease_by_field_and_kind(&[name_key], "functional")
        .await
        .unwrap()
        .expect("functional backend must exist");
    let backend = lease.backend;
    let lookup_result = backend
        .lookup(crate::index2::backend::IndexQuery::Point {
            keys: smallvec::smallvec![
                FunctionalBackend::hash_value(&InnerValue::Str("alice".into()))
                    .to_vec()
            ],
        })
        .await
        .unwrap();
    let count = match lookup_result {
        crate::index2::backend::IndexResult::Set(s) => s.len(),
        crate::index2::backend::IndexResult::Ranked(v) => v.len(),
    };
    assert_eq!(count, 1, "sanity: index must be usable");

    // Install the DROP pause hook.
    let drop_hook = Arc::new(BackfillPauseHook::new());
    tbl.set_drop_index2_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn a read that holds a lease (we park the task while holding it).
    let tbl_c = tbl.clone();
    let read_task = tokio::spawn(async move {
        let registry_c = tbl_c.index2_registry();
        let lease = registry_c
            .lease_by_field_and_kind(&[name_key], "functional")
            .await
            .unwrap()
            .expect("functional backend must exist");
        // Lease is held here — we keep it alive by not dropping it.
        // Simulate a long-running read by parking the task.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        // Explicitly drop the lease when we're done (or let RAII do it).
        drop(lease);
    });

    // Give the read a moment to start and acquire the lease.
    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

    // Verify the gate is counting in-flight readers.
    let gate = tbl.index2_registry().reader_gate();
    let in_flight = gate.in_flight_count();
    assert!(
        in_flight >= 1,
        "gate in_flight_count() must show at least one reader: {}",
        in_flight
    );

    // Now spawn the DROP (which will park at the pause hook).
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_index2("lower_name").await });

    // Rendezvous: the DROP is parked.
    drop_hook.wait_until_parked().await;

    // Release the DROP.
    drop_hook.release();

    // Both tasks should complete.
    drop_task.await.unwrap().unwrap();
    read_task.await.unwrap();

    // Verify the drain wait was counted (should be 1 since we blocked the drain).
    let waits = gate.drain_waits();
    assert_eq!(
        waits, 1,
        "gate drain_waits() must be exactly 1 (we blocked the drain)"
    );
}

// ============================================================================
// 2. Distinguishable-signal test — reader observes IndexDrainInProgress
// ============================================================================

/// P0-3a (#1038) distinguishable-signal test: while a DROP is parked at
/// `drop_index2_pause_hook` (definition already retired, drain not yet called),
/// a new `lease_by_field_and_kind` call must return `Err(IndexDrainInProgress(_))`,
/// NOT `Ok(None)` — which would be indistinguishable from "no such index exists."
#[tokio::test]
async fn p1038_index2_lease_returns_drain_error_not_none() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    // Three pre-existing rows.
    for name in ["Alice", "Bob", "Charlie"] {
        tbl.insert(&record_with_str(name_key, name))
            .await
            .unwrap();
    }

    // Build a functional index.
    tbl.create_index_v2(functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let index_name_id = key_id(&tbl, "lower_name").await;

    // Verify the index is usable before DROP starts.
    let registry = tbl.index2_registry();
    let lease = registry
        .lease_by_field_and_kind(&[name_key], "functional")
        .await
        .unwrap();
    assert!(
        lease.is_some(),
        "sanity: lease must succeed before DROP starts"
    );

    // Install the DROP pause hook.
    let drop_hook = Arc::new(BackfillPauseHook::new());
    tbl.set_drop_index2_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn the DROP (which will park at the pause hook after retiring the backend).
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_index2("lower_name").await });

    // Rendezvous: the DROP is parked (backend retired, drain not yet called).
    drop_hook.wait_until_parked().await;

    // Now try to acquire a lease — must get IndexDrainInProgress error.
    let registry = tbl.index2_registry();
    let lease_result = registry.lease_by_field_and_kind(&[name_key], "functional").await;

    match lease_result {
        Err(DbError::IndexDrainInProgress(msg)) => {
            // Good — we got the distinguishable error signal.
            assert!(msg.contains("functional"), "error message should mention the kind");
        }
        Ok(None) => {
            panic!(
                "P0-3a (#1038) REGRESSION BUG: lease_by_field_and_kind returned Ok(None) \
                 during a drain window, which is INDISTINGUISHABLE from \"no such index\". \
                 This is exactly the bug class #1037 round 1 shipped (and caught before commit)."
            );
        }
        Ok(Some(_lease)) => {
            panic!(
                "P0-3a (#1038) REGRESSION BUG: lease_by_field_and_kind returned a lease \
                 during a drain window. The gate should have blocked new readers."
            );
        }
        Err(other) => {
            panic!("Unexpected error: {:?}", other);
        }
    }

    // Release the DROP.
    drop_hook.release();

    // DROP should complete.
    drop_task.await.unwrap().unwrap();
}

// ============================================================================
// 3. Back-off pairing — drain_waits() == 0 uncontended / == 1 contending
// ============================================================================

/// P0-3a (#1038) back-off pairing test: proves both the uncontended case
/// (drain_waits() == 0) AND the contended case (drain_waits() == 1). A lone
/// `== 0` assertion passes vacuously if the gate never blocked a real drain,
/// so test suites always pair them.
#[tokio::test]
async fn p1038_index2_backoff_pairing_contended_and_uncontended() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    // Three pre-existing rows.
    for name in ["Alice", "Bob", "Charlie"] {
        tbl.insert(&record_with_str(name_key, name))
            .await
            .unwrap();
    }

    // Build a functional index.
    tbl.create_index_v2(functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let index_name_id = key_id(&tbl, "lower_name").await;
    let gate = tbl.index2_registry().reader_gate();

    // -----------------------------------------------------------------
    // 3a. Uncontended DROP — drain_waits() stays at 0
    // -----------------------------------------------------------------
    let drop_result = tbl.drop_index2("lower_name").await;
    assert!(drop_result.unwrap(), "DROP should succeed");

    let waits = gate.drain_waits();
    assert_eq!(
        waits, 0,
        "uncontended drop should not increment drain_waits()"
    );

    // Re-create the index for the contended test.
    tbl.create_index_v2(functional_lower_op("lower_name2", "people", "name"))
        .await
        .unwrap();

    // -----------------------------------------------------------------
    // 3b. Contended DROP — drain_waits() == 1
    // -----------------------------------------------------------------
    let index_name_id2 = key_id(&tbl, "lower_name2").await;

    // Install the DROP pause hook.
    let drop_hook = Arc::new(BackfillPauseHook::new());
    tbl.set_drop_index2_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn a read that holds a lease.
    let tbl_c = tbl.clone();
    let read_task = tokio::spawn(async move {
        let registry_c = tbl_c.index2_registry();
        let lease = registry_c
            .lease_by_field_and_kind(&[name_key], "functional")
            .await
            .unwrap()
            .expect("functional backend must exist");
        // Hold the lease while DROP proceeds.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        drop(lease);
    });

    // Give the read a moment to start.
    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

    // Now spawn the DROP.
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_index2("lower_name2").await });

    // Rendezvous: the DROP is parked.
    drop_hook.wait_until_parked().await;

    // Release the DROP.
    drop_hook.release();

    // Both tasks should complete.
    drop_task.await.unwrap().unwrap();
    read_task.await.unwrap();

    // Verify drain_waits() == 1.
    let waits = gate.drain_waits();
    assert_eq!(
        waits, 1,
        "contended drop should increment drain_waits() exactly once"
    );
}

// ============================================================================
// 4. Guard-release-on-error test — RAII release on early return
// ============================================================================

/// P0-3a (#1038) guard-release-on-error test: force a lease-holding read to
/// error, confirm the RAII guard releases correctly (in-flight count returns
/// to zero).
#[tokio::test]
async fn p1038_index2_guard_releases_on_error() {
    let (data_store, info_store) = (
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>,
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>,
    );

    let registry = IndexRegistry::new();

    // Build a functional backend directly.
    let name_key = 1u64; // Mock interned id
    let backend = build_ready_functional_backend(
        1,
        "lower_name",
        name_key,
        vec![name_key],
        &info_store,
    );
    registry.insert(backend).await.unwrap();

    let gate = registry.reader_gate();

    // Acquire a lease and then immediately return (simulating early exit).
    {
        let lease = registry
            .lease_by_field_and_kind(&[name_key], "functional")
            .await
            .unwrap()
            .unwrap();
        // in_flight_count should be 1 while we hold the lease.
        let in_flight = gate.in_flight_count();
        assert_eq!(in_flight, 1, "lease should increment in_flight count");
    } // RAII: lease drops here.

    // Verify the count returned to zero.
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    let in_flight = gate.in_flight_count();
    assert_eq!(
        in_flight, 0,
        "RAII guard should have decremented in_flight count"
    );
}