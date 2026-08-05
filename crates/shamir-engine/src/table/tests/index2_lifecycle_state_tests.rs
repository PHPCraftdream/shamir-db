//! F-50 Step 3b (#873) — persisted index2 lifecycle state (Building/Ready)
//! + crash/restart continuation tests.
//!
//! Three proofs:
//! 1. **Crash/restart self-heal** — a `Building` descriptor on disk (from a
//!    crash between the first `Building` persist and the final `Ready`
//!    persist) is detected on table reopen, re-backfilled from scratch, and
//!    flipped to `Ready` — automatically, no operator action.
//! 2. **Planner Ready-gate** — a `Building` backend in the LIVE registry is
//!    invisible to `try_plan_index2` (returns `None` → full scan), so no
//!    reader can observe its partial postings.
//! 3. **Doctor `Building`-detection** — `verify()` surfaces a `Building`
//!    backend as unhealthy in the new `index2_backends` report section.

use std::sync::Arc;

use shamir_query_types::admin::types::CreateIndexOp;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::backend::{IndexQuery, IndexResult};
use crate::index2::build_index2_backend_with_resolver;
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::expr::IndexExpr;
use crate::index2::functional_backend::FunctionalBackend;
use crate::index2::kind::{FunctionalConfig, IndexKind};
use crate::index2::state::IndexState;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;

/// Shared stores so that dropping a `TableManager` does NOT lose data —
/// the reopen sees the same on-disk bytes the crashed process wrote.
fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
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

/// Resolve the set of record ids the functional `lower` index holds for a
/// given (already-lowercased) value, by querying the backend directly.
async fn functional_lookup(tbl: &TableManager, index_name_id: u64, lowered: &str) -> Vec<[u8; 16]> {
    let backend = tbl
        .index2_registry()
        .get_by_name(index_name_id)
        .await
        .expect("functional backend must be registered");
    let key = FunctionalBackend::hash_value(&InnerValue::Str(lowered.into()));
    let mut keys: smallvec::SmallVec<[Vec<u8>; 4]> = smallvec::SmallVec::new();
    keys.push(key.to_vec());
    match backend.lookup(IndexQuery::Point { keys }).await.unwrap() {
        IndexResult::Set(s) => s.iter().map(|rid| *rid.as_bytes()).collect(),
        IndexResult::Ranked(v) => v.iter().map(|(rid, _)| *rid.as_bytes()).collect(),
    }
}

/// Build a functional `lower(<field>)` index2 backend with `state = Building`
/// (directly — NOT through `create_index_v2`, which would flip to Ready).
fn build_building_functional_backend(
    id: u32,
    name: &str,
    name_interned: u64,
    field_path: Vec<u64>,
    info_store: &Arc<dyn Store>,
) -> Arc<dyn crate::index2::backend::IndexBackend> {
    let first_path = field_path.clone();
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Field(first_path)));
    let kind = IndexKind::Functional(Box::new(FunctionalConfig { expr }));
    let mut desc = IndexDescriptor::new(
        id,
        name,
        name_interned,
        smallvec::smallvec![field_path],
        kind,
    );
    desc.state = IndexState::Building;
    build_index2_backend_with_resolver(desc, info_store, None)
}

// ============================================================================
// 1. Crash/restart self-heal — Building → Ready on table reopen
// ============================================================================

/// Simulate a crash mid-`create_index_v2` (the first `Building` persist ran,
/// the backfill completed, but the final `Ready` persist never did) by parking
/// the create at the post-backfill / pre-register hook point, then aborting
/// the task. On reopen, the `Building` descriptor must self-heal: drop the
/// partial postings, re-run the full backfill, flip to `Ready`, and re-persist.
#[tokio::test]
async fn building_index_self_heals_on_reopen() {
    let (data_store, info_store) = make_stores();

    // --- Phase 1: create + park mid-build (Building persisted on disk). ---
    let mgr1 = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let name_field = key_id(&mgr1, "name").await;
    mgr1.insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();
    mgr1.insert(&record_with_str(name_field, "Bob"))
        .await
        .unwrap();
    // Persist the interner so the interned field-name id survives reopen.
    mgr1.interner().persist().await.unwrap();

    // Install the deterministic pause hook (parks create between backfill
    // and register — the exact crash window).
    let hook = Arc::new(BackfillPauseHook::new());
    mgr1.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));

    let mgr1c = mgr1.clone();
    let create = tokio::spawn(async move {
        mgr1c
            .create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
    });

    // Wait until the create is parked: backfill done (postings written under
    // the reserved id), backend NOT yet registered, final save NOT yet run.
    // The first `save_index2_metadata_with_pending` (Building descriptor)
    // HAS run — so Building is durable on disk.
    hook.wait_until_parked().await;

    // Simulate crash: abort the parked create task and drop the manager.
    create.abort();
    let _ = create.await;
    drop(mgr1);

    // --- Phase 2: reopen on the same stores. ---
    let mgr2 = TableManager::create(
        "people".into(),
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // The Building backend must have self-healed to Ready.
    let backends = mgr2.index2_registry().all_backends().await;
    assert_eq!(
        backends.len(),
        1,
        "expected exactly 1 index2 backend after reopen, got {}",
        backends.len()
    );
    let backend = &backends[0];
    let state = mgr2
        .index2_registry()
        .state_of(backend.descriptor().id)
        .await;
    assert_eq!(
        state,
        Some(IndexState::Ready),
        "Building backend must self-heal to Ready on reopen (got {:?})",
        state
    );

    // The index must be queryable — the open-path backfill re-ran from scratch.
    let ln_id = key_id(&mgr2, "lower_name").await;
    let alice = functional_lookup(&mgr2, ln_id, "alice").await;
    assert_eq!(
        alice.len(),
        1,
        "Alice must be indexed after self-heal backfill"
    );
    let bob = functional_lookup(&mgr2, ln_id, "bob").await;
    assert_eq!(bob.len(), 1, "Bob must be indexed after self-heal backfill");

    // The persisted metadata must now reflect Ready (the open path re-persisted).
    let persisted = crate::index2::persistence::load_index2_metadata(&info_store)
        .await
        .unwrap()
        .expect("metadata must exist after reopen");
    assert_eq!(persisted.descriptors.len(), 1);
    assert_eq!(
        persisted.descriptors[0].state,
        IndexState::Ready,
        "persisted descriptor must be Ready after self-heal re-persist"
    );
}

// ============================================================================
// 2. Planner Ready-gate — a Building backend is invisible to try_plan_index2
// ============================================================================

/// A `Building` functional backend exists in the LIVE registry, but
/// `try_plan_index2` returns `None` for a matching `Computed` filter — the
/// `find_by_field_and_kind` Ready-gate skips it, so reads fall through to a
/// full scan instead of observing partial postings.
#[tokio::test]
async fn planner_ready_gate_hides_building_backend() {
    use crate::query::filter::{Filter, FilterValue};

    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();

    // Intern the field name so the planner's `intern_field_path` resolves.
    let name_field = key_id(&mgr, "name").await;
    let name_key = key_id(&mgr, "lower_name").await;

    // Build + insert a Building functional backend directly (NOT via
    // create_index_v2, which would flip it to Ready).
    let backend = build_building_functional_backend(
        1,
        "lower_name",
        name_key,
        vec![name_field],
        &mgr.info_store,
    );
    mgr.index2_registry().insert(backend).await.unwrap();

    // Sanity: the backend IS in the registry.
    assert_eq!(mgr.index2_registry().len(), 1);
    assert_eq!(
        mgr.index2_registry().state_of(1).await,
        Some(IndexState::Building),
        "backend must be in Building state"
    );

    // The planner must NOT route to the Building backend.
    let filter = Filter::Computed {
        expr_op: "lower".into(),
        field: vec!["name".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice".into()),
    };
    let interner = mgr.interner().get().await.unwrap();
    let plan = mgr.try_plan_index2(&filter, interner).await;
    assert!(
        plan.is_none(),
        "a Building backend must be invisible to the planner (got {:?})",
        plan.is_some()
    );

    // Flip to Ready and verify the planner NOW finds it.
    mgr.index2_registry().set_state(1, IndexState::Ready).await;
    let plan_ready = mgr.try_plan_index2(&filter, interner).await;
    assert!(
        plan_ready.is_some(),
        "after flipping to Ready, the planner must find the backend"
    );
}

// ============================================================================
// 3. Doctor Building-detection — verify() reports a Building backend unhealthy
// ============================================================================

/// `verify()` surfaces a `Building` backend in the new `index2_backends`
/// report section as `healthy == false` with the diagnostic message, and the
/// overall `is_healthy()` reflects it.
#[tokio::test]
async fn doctor_detects_building_index2_backend() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();

    let name_field = key_id(&mgr, "name").await;
    let name_key = key_id(&mgr, "lower_name").await;

    // Build + insert a Building functional backend.
    let backend = build_building_functional_backend(
        1,
        "lower_name",
        name_key,
        vec![name_field],
        &mgr.info_store,
    );
    mgr.index2_registry().insert(backend).await.unwrap();

    let report = mgr.verify().await.unwrap();

    // The index2_backends section must contain the Building entry.
    assert_eq!(
        report.index2_backends.len(),
        1,
        "expected 1 index2 backend in the report"
    );
    let health = &report.index2_backends[0];
    assert_eq!(health.id, 1);
    assert_eq!(health.name, "lower_name");
    assert_eq!(health.state, IndexState::Building);
    assert!(
        !health.healthy,
        "a Building backend must be reported as unhealthy"
    );
    let msg = health
        .message
        .as_ref()
        .expect("unhealthy entry must carry a diagnostic message");
    assert!(
        msg.contains("Building state"),
        "message must mention Building state, got: {msg}"
    );
    assert!(
        msg.contains("lower_name"),
        "message must name the index, got: {msg}"
    );

    // The overall report must be unhealthy (Building folded into is_healthy).
    assert!(
        !report.is_healthy(),
        "a Building backend must make the overall report unhealthy"
    );
    assert!(
        !report.all_indexes_healthy(),
        "all_indexes_healthy must be false with a Building backend"
    );

    // Flip to Ready and verify the report is now healthy.
    mgr.index2_registry().set_state(1, IndexState::Ready).await;
    let report2 = mgr.verify().await.unwrap();
    assert!(
        report2.index2_backends[0].healthy,
        "after flipping to Ready, the backend must be healthy"
    );
    assert!(
        report2.index2_backends[0].message.is_none(),
        "a healthy backend must have no diagnostic message"
    );
}

/// R0-D (#1013): `verify()` surfaces a `Failed` backend as unhealthy, with
/// the recorded failure reason (not just the enum variant) folded into the
/// diagnostic message — this is the concrete proof of the brief's "propagate
/// the underlying error string into the entry" requirement.
#[tokio::test]
async fn doctor_detects_failed_index2_backend_with_reason() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();

    let name_field = key_id(&mgr, "name").await;
    let name_key = key_id(&mgr, "lower_name").await;

    let backend = build_building_functional_backend(
        1,
        "lower_name",
        name_key,
        vec![name_field],
        &mgr.info_store,
    );
    mgr.index2_registry().insert(backend).await.unwrap();

    // Simulate the table-open recovery path failing this backend closed.
    mgr.index2_registry()
        .set_failed(1, "restore_on_open failed: injected disk fault xyz")
        .await;

    let report = mgr.verify().await.unwrap();
    assert_eq!(report.index2_backends.len(), 1);
    let health = &report.index2_backends[0];
    assert_eq!(health.state, IndexState::Failed);
    assert!(
        !health.healthy,
        "a Failed backend must be reported unhealthy"
    );
    let msg = health
        .message
        .as_ref()
        .expect("a Failed backend must carry a diagnostic message");
    assert!(
        msg.contains("Failed state"),
        "message must mention Failed state, got: {msg}"
    );
    assert!(
        msg.contains("injected disk fault xyz"),
        "message must propagate the UNDERLYING failure reason, not just the \
         enum variant, got: {msg}"
    );
    assert!(
        !report.is_healthy(),
        "a Failed backend must make the overall report unhealthy"
    );
}
