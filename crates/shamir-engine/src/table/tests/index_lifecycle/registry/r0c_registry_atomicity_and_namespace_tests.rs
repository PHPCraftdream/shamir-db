//! R0-C (#1009 + #1010) — `IndexRegistry::insert` atomicity, open-path
//! fail-closed on duplicate persisted names, cross-family index namespace
//! uniqueness, and the `doctor::verify()` checks for both.
//!
//! `IndexRegistry`-level unit tests for the #1009 `insert()` atomicity fix
//! itself (a colliding name must leave `by_id` untouched) live in
//! `shamir-index`'s `tests/registry_tests.rs` — this file covers the
//! engine-level surfaces: `TableManager::create`'s open-path fail-closed
//! behavior on corrupt (duplicate-name) persisted metadata, the four CREATE
//! entry points' cross-family preflight, and `doctor::verify()`'s two new
//! consistency checks.

use std::sync::Arc;

use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;

use crate::index2::descriptor::IndexDescriptor;
use crate::index2::kind::{FunctionalConfig, IndexKind};
use crate::index2::persistence::PersistedIndexes;
use crate::index2::state::IndexState;
use crate::index2::{IndexExpr, MetaEnvelope};
use crate::table::TableManager;

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

/// A minimal, well-formed functional-index descriptor. `field_key` stands in
/// for an already-interned field path segment (these tests never backfill
/// this descriptor through a live table, so the field id need not resolve to
/// anything real unless noted otherwise).
fn functional_descriptor(
    id: u32,
    name: &str,
    name_interned: u64,
    field_key: u64,
    state: IndexState,
) -> IndexDescriptor {
    let mut desc = IndexDescriptor::new(
        id,
        name,
        name_interned,
        smallvec::smallvec![vec![field_key]],
        IndexKind::Functional(Box::new(FunctionalConfig {
            expr: IndexExpr::Lower(Box::new(IndexExpr::Field(vec![field_key]))),
        })),
    );
    desc.state = state;
    desc
}

/// Directly write a `PersistedIndexes` blob to `info_store`, bypassing
/// `IndexRegistry`/`save_index2_metadata` entirely — this is how the tests
/// synthesize the "on-disk metadata is corrupt" scenario (two descriptors
/// sharing a name), which `IndexRegistry::insert`'s #1009 check-before-mutate
/// fix now makes UNREACHABLE through any normal (non-corrupted) code path.
async fn write_raw_persisted_indexes(
    info_store: &Arc<dyn Store>,
    next_id: u32,
    descriptors: Vec<IndexDescriptor>,
) {
    let p = PersistedIndexes {
        next_id,
        descriptors,
    };
    let envelope = MetaEnvelope::new(p);
    let bytes = envelope.encode().unwrap();
    let key = shamir_types::types::record_id::RecordId::system("_m.idx").to_bytes();
    info_store
        .set(key.into(), bytes::Bytes::from(bytes))
        .await
        .unwrap();
}

// ============================================================================
// Part 1 (#1009) — open-path recovery must fail the WHOLE table open on
// duplicate persisted index2 names, not silently drop one descriptor.
// ============================================================================

/// Two persisted `Ready` index2 descriptors sharing the SAME `name_interned`
/// (distinct ids) is exactly the "on-disk metadata is contradictory"
/// scenario #1009's fix defines. Before the fix, `IndexRegistry::insert`
/// silently swallowed (`let _ = ... .insert(...).await;`) the second
/// descriptor's failure and `TableManager::create` returned `Ok` with a
/// SILENTLY partially-loaded table (only the first-inserted descriptor
/// registered). Post-fix, the second `insert()` call returns `Err` (name
/// already taken) and the three open-path call sites now propagate that
/// `Err` out of `TableManager::create` instead of discarding it.
///
/// This test fails against the pre-fix code: `TableManager::create` would
/// return `Ok(mgr)` with exactly ONE of the two colliding descriptors
/// registered (whichever was inserted first) — the `expect_err` below
/// would panic on an `Ok` value.
#[tokio::test]
async fn duplicate_persisted_index2_name_fails_whole_table_open() {
    let (data_store, info_store) = make_stores();

    let field_key = 42u64;
    let name_interned = 777u64;
    let d1 = functional_descriptor(1, "dup_name", name_interned, field_key, IndexState::Ready);
    let d2 = functional_descriptor(2, "dup_name", name_interned, field_key, IndexState::Ready);
    write_raw_persisted_indexes(&info_store, 3, vec![d1, d2]).await;

    let result = TableManager::create("docs".into(), data_store, info_store).await;

    let err = match result {
        Ok(_) => panic!(
            "#1009: TableManager::create must fail the WHOLE table open when \
             persisted index2 metadata contains two descriptors sharing a name — \
             not silently proceed with a partially-loaded table"
        ),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("dup_name") || msg.contains("777"),
        "the error must name the colliding descriptor (name or id): got {msg}"
    );
}

/// Same defect, but the SECOND colliding descriptor is persisted in
/// `Building` state — this exercises the OTHER two open-path call sites (the
/// Building self-heal branch), not just the plain-insert branch the sibling
/// test above covers. `TableManager::create` must still fail the whole open;
/// the fact that the descriptor happens to be mid-backfill does not make a
/// duplicate name any less contradictory.
#[tokio::test]
async fn duplicate_persisted_index2_name_with_building_descriptor_fails_whole_open() {
    let (data_store, info_store) = make_stores();

    let field_key = 42u64;
    let name_interned = 888u64;
    let d1 = functional_descriptor(
        1,
        "dup_building",
        name_interned,
        field_key,
        IndexState::Ready,
    );
    let d2 = functional_descriptor(
        2,
        "dup_building",
        name_interned,
        field_key,
        IndexState::Building,
    );
    write_raw_persisted_indexes(&info_store, 3, vec![d1, d2]).await;

    let result = TableManager::create("docs".into(), data_store, info_store).await;

    assert!(
        result.is_err(),
        "#1009: a duplicate name must fail the whole table open even when the \
         colliding descriptor is in Building state (the self-heal recovery branch)"
    );
}

/// Sanity/negative control: two DISTINCT names must open normally — proves
/// the fail-closed behavior above is specific to the name collision, not a
/// blanket regression against ordinary multi-descriptor tables.
#[tokio::test]
async fn distinct_persisted_index2_names_open_normally() {
    let (data_store, info_store) = make_stores();

    let field_key = 42u64;
    let d1 = functional_descriptor(1, "name_a", 111, field_key, IndexState::Ready);
    let d2 = functional_descriptor(2, "name_b", 222, field_key, IndexState::Ready);
    write_raw_persisted_indexes(&info_store, 3, vec![d1, d2]).await;

    let mgr = TableManager::create("docs".into(), data_store, info_store)
        .await
        .expect("two descriptors with distinct names must open successfully");
    let backends = mgr.index2_registry().all_backends().await;
    assert_eq!(
        backends.len(),
        2,
        "both distinct-named backends must be registered"
    );
}

// ============================================================================
// Part 1 (#1009) — doctor::verify()'s by_id <-> by_name <-> persisted
// consistency check.
// ============================================================================

/// With no corruption, `verify()`'s new index2 registry consistency check
/// must report zero problems (the common case).
#[tokio::test]
async fn verify_reports_no_registry_inconsistency_on_healthy_table() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("docs".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "title").await;

    let report = mgr.verify().await.unwrap();
    assert!(
        report.index2_registry_consistency.is_empty(),
        "a healthy (empty) table must report zero index2 registry \
         inconsistencies, got {:?}",
        report.index2_registry_consistency
    );
}

/// `verify()`'s consistency check must flag a live-vs-persisted drift: a
/// backend registered in memory whose persisted counterpart has since
/// diverged. Constructed by inserting a backend into the live registry and
/// then persisting a DIFFERENT descriptor (different `name_interned`) under
/// the same id directly to disk — this is the "registry has drifted from
/// its own persisted snapshot" case the check's doc describes; post-#1009
/// `insert()` no longer allows this to happen via `by_id`/`by_name`
/// themselves (that half is proven unreachable by the sibling
/// `shamir-index` registry tests), so this test exercises the check's
/// live-vs-persisted half directly via the internal seam (writing the
/// persisted blob independently of the live registry).
#[tokio::test]
async fn verify_detects_live_vs_persisted_drift() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("docs".into(), data_store, info_store)
        .await
        .unwrap();
    let field_key = key_id(&mgr, "title").await;

    mgr.create_index_v2(&functional_index_op("drift_idx", "docs", "title"))
        .await
        .unwrap();
    let live_id = mgr.index2_registry().all_backends().await[0]
        .descriptor()
        .id;

    // Persist a DIFFERENT descriptor (different name_interned) under the
    // SAME id, bypassing `save_index2_metadata` — simulates the registry
    // having drifted from what's actually durable (e.g. a skipped persist
    // after a rename).
    let drifted = functional_descriptor(
        live_id,
        "renamed_elsewhere",
        999_999,
        field_key,
        IndexState::Ready,
    );
    write_raw_persisted_indexes(mgr.info_store(), live_id + 1, vec![drifted]).await;

    let report = mgr.verify().await.unwrap();
    assert!(
        !report.index2_registry_consistency.is_empty(),
        "verify() must detect that the live registry entry and the persisted \
         descriptor for the same id disagree on name_interned"
    );
}

// ============================================================================
// Part 2 (#1010) — cross-family index name uniqueness at CREATE time.
// ============================================================================

fn functional_index_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
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

/// regular (existing) -> index2 (new, same name) must be rejected.
#[tokio::test]
async fn create_index2_rejects_name_taken_by_regular() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "city").await;
    mgr.create_index("shared_name", &["city"]).await.unwrap();

    let err = mgr
        .create_index_v2(&functional_index_op("shared_name", "t", "city"))
        .await
        .expect_err(
            "#1010: CREATE for index2 must reject a name already used by the \
             regular family",
        );
    assert!(err.to_string().contains("shared_name"));
}

/// unique (existing) -> sorted (new, same name) must be rejected.
#[tokio::test]
async fn create_sorted_rejects_name_taken_by_unique() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "id").await;
    let _ = key_id(&mgr, "score").await;
    mgr.create_unique_index("shared_name", &["id"])
        .await
        .unwrap();

    let err = mgr
        .create_sorted_index("shared_name", &["score"])
        .await
        .expect_err(
            "#1010: CREATE SORTED INDEX must reject a name already used by the \
             unique family",
        );
    assert!(err.to_string().contains("shared_name"));
}

/// sorted (existing) -> unique (new, same name) must be rejected.
#[tokio::test]
async fn create_unique_rejects_name_taken_by_sorted() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "score").await;
    let _ = key_id(&mgr, "id").await;
    mgr.create_sorted_index("shared_name", &["score"])
        .await
        .unwrap();

    let err = mgr
        .create_unique_index("shared_name", &["id"])
        .await
        .expect_err(
            "#1010: CREATE UNIQUE INDEX must reject a name already used by the \
             sorted family",
        );
    assert!(err.to_string().contains("shared_name"));
}

/// index2 (existing) -> regular (new, same name) must be rejected.
#[tokio::test]
async fn create_regular_rejects_name_taken_by_index2() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "city").await;
    mgr.create_index_v2(&functional_index_op("shared_name", "t", "city"))
        .await
        .unwrap();

    let err = mgr.create_index("shared_name", &["city"]).await.expect_err(
        "#1010: CREATE INDEX (regular) must reject a name already used by the \
             index2 family",
    );
    assert!(err.to_string().contains("shared_name"));
}

/// Negative control: creating indexes with genuinely distinct names across
/// all four families must all succeed — proves the preflight is scoped to
/// actual collisions, not a blanket rejection.
#[tokio::test]
async fn create_with_distinct_names_across_families_all_succeed() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "city").await;
    let _ = key_id(&mgr, "id").await;
    let _ = key_id(&mgr, "score").await;

    mgr.create_index("regular_idx", &["city"]).await.unwrap();
    mgr.create_unique_index("unique_idx", &["id"])
        .await
        .unwrap();
    mgr.create_sorted_index("sorted_idx", &["score"])
        .await
        .unwrap();
    mgr.create_index_v2(&functional_index_op("index2_idx", "t", "city"))
        .await
        .unwrap();
}

// ============================================================================
// Part 2 (#1010) — doctor::verify()'s cross-family collision check.
// ============================================================================

/// `verify()` must report zero collisions on a healthy table (all four
/// families created with distinct names).
#[tokio::test]
async fn verify_reports_no_cross_family_collision_on_healthy_table() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let _ = key_id(&mgr, "city").await;
    mgr.create_index("regular_idx", &["city"]).await.unwrap();

    let report = mgr.verify().await.unwrap();
    assert!(
        report.cross_family_name_collisions.is_empty(),
        "a healthy table must report zero cross-family collisions, got {:?}",
        report.cross_family_name_collisions
    );
    assert!(report.is_healthy());
}

/// `verify()` must detect a PRE-EXISTING cross-family collision that could
/// not have been created through the (now-guarded) CREATE paths. Simulated
/// by registering a sorted-index definition directly through
/// `SortedIndexManager::register` (which has no cross-family guard of its
/// own — the #1010 guard lives one layer up, in
/// `create_sorted_index_with_include`) under the SAME `name_interned` as an
/// already-existing regular index — exactly mirroring a collision left over
/// from before the #1010 fix landed (or any future regression that bypasses
/// the `TableManager::create_*` preflight).
#[tokio::test]
async fn verify_detects_pre_existing_cross_family_collision() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let city_key = key_id(&mgr, "city").await;
    mgr.create_index("shared_name", &["city"]).await.unwrap();

    let name_interned = {
        let interner = mgr.interner().get().await.unwrap();
        match interner.touch_ind("shared_name").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    };
    let sorted_def = crate::index::sorted_index_manager::SortedIndexDefinition::new(
        name_interned,
        vec![city_key],
    );
    mgr.sorted_indexes().register(sorted_def).await.unwrap();

    let report = mgr.verify().await.unwrap();
    assert!(
        !report.cross_family_name_collisions.is_empty(),
        "verify() must detect the pre-existing cross-family collision on \
         'shared_name' (regular + sorted)"
    );
    assert!(
        report
            .cross_family_name_collisions
            .iter()
            .any(|s| s.contains("shared_name") || s.contains(&name_interned.to_string())),
        "the diagnostic must name the colliding index: {:?}",
        report.cross_family_name_collisions
    );
    assert!(
        !report.is_healthy(),
        "a collision must make the overall report unhealthy"
    );
}
