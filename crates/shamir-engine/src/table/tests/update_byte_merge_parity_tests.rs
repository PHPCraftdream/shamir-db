//! Byte/op-identity and behavioural parity tests for the W3 byte-merge
//! update path (`merge_storage_bytes` + `update_tx_bytes`).
//!
//! Four guarantees:
//!
//! 1. **Storage-bytes identity**: `merge_storage_bytes(old_bytes, set_map)` ==
//!    `merge_inner_maps(old_tree, set_map).to_bytes()` across a battery of
//!    shapes (overlap, new-key, nested, type-change, no-op).
//!
//! 2. **Index-op identity**: `plan_update_ops_ref(&old_view, &new_view)` ==
//!    `plan_update_ops(&old_tree, &new_tree)` (sorted op vecs).
//!
//! 3. **Change-detection identity**: `(new_bytes == old_bytes)` iff
//!    `(merge_inner_maps(old, set) == old)`.
//!
//! 4. **Behavioural**: UPDATE an indexed field reflects in the index;
//!    UPDATE introducing a new field + returning works.

use std::sync::Arc;

use bytes::Bytes;
use shamir_query_builder::write::{doc, UpdateReturnMode};
use shamir_query_builder::{filter, write as w};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::codecs::interned::merge_storage_bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::table_manager::TableManager;
use crate::table::tests::write_exec_tests::{insert_via_tx, setup_empty_table, update_via_tx};

// ============================================================================
// Helpers
// ============================================================================

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

/// Build an InnerValue map with the given (string_key, InnerValue) pairs,
/// interning through the table's interner. Returns the tree + the set_map
/// (interned-key → InnerValue) that the merge pipeline uses.
async fn make_record(
    tbl: &TableManager,
    fields: &[(&str, InnerValue)],
) -> (InnerValue, TMap<InternerKey, InnerValue>) {
    let interner = tbl.interner().get().await.unwrap();
    let mut tree_map: TMap<InternerKey, InnerValue> = new_map();
    for (k, v) in fields {
        let ik = interner.touch_ind(k).unwrap().into_key();
        tree_map.insert(ik, v.clone());
    }
    tbl.interner().persist().await.unwrap();
    (InnerValue::Map(tree_map.clone()), tree_map)
}

/// Seed a record into the table, returning (rid, raw_storage_bytes, tree).
async fn seed_record(
    tbl: &TableManager,
    fields: &[(&str, InnerValue)],
) -> (RecordId, Bytes, InnerValue) {
    let (tree, _) = make_record(tbl, fields).await;
    let rid = tbl.insert(&tree).await.unwrap();
    let raw_bytes = tbl.data_store().get(rid.to_bytes()).await.unwrap();
    (rid, raw_bytes, tree)
}

/// The tree-path merge (reference implementation).
fn merge_inner_maps(original: &InnerValue, set_map: &TMap<InternerKey, InnerValue>) -> InnerValue {
    match original {
        InnerValue::Map(orig_map) => {
            let mut merged = orig_map.clone();
            for (key, value) in set_map {
                merged.insert(key.clone(), value.clone());
            }
            InnerValue::Map(merged)
        }
        _ => original.clone(),
    }
}

fn op_to_sort_key(op: &shamir_tx::IndexWriteOp) -> Vec<u8> {
    match op {
        shamir_tx::IndexWriteOp::SetPosting { key, value } => {
            let mut v = b"set:".to_vec();
            v.extend_from_slice(key);
            v.push(b'|');
            v.extend_from_slice(value);
            v
        }
        shamir_tx::IndexWriteOp::RemovePosting { key } => {
            let mut v = b"rm:".to_vec();
            v.extend_from_slice(key);
            v
        }
        shamir_tx::IndexWriteOp::BumpFtsStats { .. } => b"fts:".to_vec(),
    }
}

fn sort_ops(ops: &mut [shamir_tx::IndexWriteOp]) {
    ops.sort_by_key(op_to_sort_key);
}

fn ops_as_sortkeys(ops: &[shamir_tx::IndexWriteOp]) -> Vec<Vec<u8>> {
    ops.iter().map(op_to_sort_key).collect()
}

// ============================================================================
// 1. Storage-bytes identity: merge_storage_bytes == tree merge + to_bytes
// ============================================================================

/// Helper: asserts byte-identity between the byte-merge and tree-merge paths.
async fn assert_storage_bytes_identity(
    tbl: &TableManager,
    base_fields: &[(&str, InnerValue)],
    set_fields: &[(&str, InnerValue)],
    label: &str,
) {
    let (_, raw_bytes, old_tree) = seed_record(tbl, base_fields).await;
    let (_, set_map) = make_record(tbl, set_fields).await;

    // Byte-merge path.
    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();

    // Tree-merge path.
    let merged_tree = merge_inner_maps(&old_tree, &set_map);
    let tree_bytes = merged_tree.to_bytes().unwrap();

    assert_eq!(
        new_bytes.as_ref(),
        tree_bytes.as_ref(),
        "storage-bytes identity failed for: {label}"
    );
}

#[tokio::test]
async fn update_storage_bytes_identity_overlap() {
    let tbl = make_table().await;
    assert_storage_bytes_identity(
        &tbl,
        &[
            ("name", InnerValue::Str("Alice".into())),
            ("age", InnerValue::Int(30)),
        ],
        &[("age", InnerValue::Int(31))],
        "overlap: update existing field",
    )
    .await;
}

#[tokio::test]
async fn update_storage_bytes_identity_new_key() {
    let tbl = make_table().await;
    assert_storage_bytes_identity(
        &tbl,
        &[("name", InnerValue::Str("Alice".into()))],
        &[("email", InnerValue::Str("alice@example.com".into()))],
        "new key: add a field",
    )
    .await;
}

#[tokio::test]
async fn update_storage_bytes_identity_nested() {
    let tbl = make_table().await;
    let interner = tbl.interner().get().await.unwrap();
    let city_k = interner.touch_ind("city").unwrap().into_key();
    let mut addr_map: TMap<InternerKey, InnerValue> = new_map();
    addr_map.insert(city_k, InnerValue::Str("NYC".into()));
    let nested = InnerValue::Map(addr_map);

    assert_storage_bytes_identity(
        &tbl,
        &[
            ("name", InnerValue::Str("Alice".into())),
            ("addr", nested.clone()),
        ],
        &[(
            "addr",
            InnerValue::Map({
                let interner = tbl.interner().get().await.unwrap();
                let city_k = interner.touch_ind("city").unwrap().into_key();
                let mut m: TMap<InternerKey, InnerValue> = new_map();
                m.insert(city_k, InnerValue::Str("LA".into()));
                m
            }),
        )],
        "nested: update nested map field",
    )
    .await;
}

#[tokio::test]
async fn update_storage_bytes_identity_type_change() {
    let tbl = make_table().await;
    assert_storage_bytes_identity(
        &tbl,
        &[("x", InnerValue::Int(42))],
        &[("x", InnerValue::Str("hello".into()))],
        "type change: Int -> Str",
    )
    .await;
}

#[tokio::test]
async fn update_storage_bytes_identity_noop() {
    let tbl = make_table().await;
    assert_storage_bytes_identity(
        &tbl,
        &[
            ("name", InnerValue::Str("Alice".into())),
            ("age", InnerValue::Int(30)),
        ],
        &[("age", InnerValue::Int(30))],
        "no-op: set same value",
    )
    .await;
}

// ============================================================================
// 2. Index-op identity: plan_update_ops_ref (RecordView) == plan_update_ops (tree)
// ============================================================================

#[tokio::test]
async fn update_index_ops_view_eq_tree_no_backends() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) = seed_record(
        &tbl,
        &[
            ("city", InnerValue::Str("NYC".into())),
            ("score", InnerValue::Int(42)),
        ],
    )
    .await;
    let (_, set_map) = make_record(&tbl, &[("city", InnerValue::Str("LA".into()))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let new_tree = merge_inner_maps(&old_tree, &set_map);

    let old_view = RecordView::new(&raw_bytes).unwrap();
    let new_view = RecordView::new(&new_bytes).unwrap();
    let rid = RecordId::new();
    let tx_id = Some(shamir_tx::TxId::new(1));

    // Index2 backends (empty, but proves generic dispatch works).
    let mut ops_tree = tbl
        .plan_update_ops(rid, &old_tree, &new_tree, tx_id)
        .await
        .unwrap();
    let mut ops_view = tbl
        .plan_update_ops_ref(rid, &old_view, &new_view, tx_id)
        .await
        .unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);
    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_update_ops: RecordView and InnerValue must produce identical ops"
    );
}

#[tokio::test]
async fn update_legacy_ops_view_eq_tree_regular_index() {
    let tbl = make_table().await;
    tbl.create_index("city_idx", &["city"]).await.unwrap();

    let (_, raw_bytes, old_tree) = seed_record(
        &tbl,
        &[
            ("city", InnerValue::Str("NYC".into())),
            ("score", InnerValue::Int(42)),
        ],
    )
    .await;
    let (_, set_map) = make_record(&tbl, &[("city", InnerValue::Str("LA".into()))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let new_tree = merge_inner_maps(&old_tree, &set_map);

    let old_view = RecordView::new(&raw_bytes).unwrap();
    let new_view = RecordView::new(&new_bytes).unwrap();
    let rid = RecordId::new();

    let mut ops_tree = tbl
        .plan_legacy_update_ops(rid, &old_tree, &new_tree)
        .await
        .unwrap();
    let mut ops_view = tbl
        .plan_legacy_update_ops_ref(rid, &old_view, &new_view)
        .await
        .unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);
    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_legacy_update_ops (regular index): RecordView and InnerValue must agree"
    );
    // Should have ops (RemovePosting for old + SetPosting for new).
    assert!(
        !ops_tree.is_empty(),
        "expected non-empty index ops for a value change on an indexed field"
    );
}

// ============================================================================
// 3. Change-detection identity: byte-eq iff tree-eq
// ============================================================================

#[tokio::test]
async fn update_change_detection_noop() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) = seed_record(
        &tbl,
        &[
            ("name", InnerValue::Str("Alice".into())),
            ("age", InnerValue::Int(30)),
        ],
    )
    .await;
    // Set the same value → no change.
    let (_, set_map) = make_record(&tbl, &[("age", InnerValue::Int(30))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);

    let bytes_changed = new_bytes.as_ref() != raw_bytes.as_ref();
    let tree_changed = merged_tree != old_tree;

    assert_eq!(
        bytes_changed, tree_changed,
        "change-detection divergence on no-op set: bytes={bytes_changed}, tree={tree_changed}"
    );
    assert!(!bytes_changed, "no-op set must detect no change");
}

#[tokio::test]
async fn update_change_detection_value_change() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) = seed_record(
        &tbl,
        &[
            ("name", InnerValue::Str("Alice".into())),
            ("age", InnerValue::Int(30)),
        ],
    )
    .await;
    let (_, set_map) = make_record(&tbl, &[("age", InnerValue::Int(31))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);

    let bytes_changed = new_bytes.as_ref() != raw_bytes.as_ref();
    let tree_changed = merged_tree != old_tree;

    assert_eq!(
        bytes_changed, tree_changed,
        "change-detection divergence on value change: bytes={bytes_changed}, tree={tree_changed}"
    );
    assert!(bytes_changed, "value change must be detected");
}

#[tokio::test]
async fn update_change_detection_new_key() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) =
        seed_record(&tbl, &[("name", InnerValue::Str("Alice".into()))]).await;
    let (_, set_map) = make_record(&tbl, &[("email", InnerValue::Str("a@b.c".into()))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);

    let bytes_changed = new_bytes.as_ref() != raw_bytes.as_ref();
    let tree_changed = merged_tree != old_tree;

    assert_eq!(
        bytes_changed, tree_changed,
        "change-detection divergence on new key: bytes={bytes_changed}, tree={tree_changed}"
    );
    assert!(bytes_changed, "adding a new key must be detected as change");
}

// ============================================================================
// 4. Behavioural: UPDATE via the production implicit-tx path
// ============================================================================

/// UPDATE an indexed field → index reflects the new value.
#[tokio::test]
async fn update_indexed_field_reflected_in_index() {
    let (table, repo) = setup_empty_table().await;
    table.create_index("city_idx", &["city"]).await.unwrap();

    let refs = new_map();

    // Insert a record with city=NYC.
    let insert_op = w::insert("users")
        .row(doc().set("name", "Alice").set("city", "NYC"))
        .build();
    insert_via_tx(&repo, &table, &insert_op, false)
        .await
        .unwrap();

    // Update city=NYC → city=LA.
    let update_op = w::update("users")
        .where_(filter::eq("city", "NYC"))
        .set(doc().set("city", "LA"))
        .returning(UpdateReturnMode::Changed)
        .build();
    let result = update_via_tx(&repo, &table, &update_op, &refs)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);
    assert_eq!(
        result.records[0].get_value_owned("city"),
        Some(shamir_types::types::value::QueryValue::Str(
            "LA".to_string()
        ))
    );

    // The old posting (NYC) should yield no results.
    let find_nyc = w::update("users")
        .where_(filter::eq("city", "NYC"))
        .set(doc().set("dummy", true))
        .build();
    let nyc_result = update_via_tx(&repo, &table, &find_nyc, &refs)
        .await
        .unwrap();
    assert_eq!(
        nyc_result.affected, 0,
        "old index entry for NYC should be gone"
    );

    // The new posting (LA) should find the record.
    let find_la = w::update("users")
        .where_(filter::eq("city", "LA"))
        .set(doc().set("dummy", true))
        .returning(UpdateReturnMode::Changed)
        .build();
    let la_result = update_via_tx(&repo, &table, &find_la, &refs).await.unwrap();
    assert_eq!(la_result.affected, 1, "new index entry for LA should exist");
}

/// UPDATE introducing a new field + returning (the W3a result pattern).
#[tokio::test]
async fn update_new_field_with_returning() {
    let (table, repo) = setup_empty_table().await;
    let refs = new_map();

    // Insert a record with just name.
    let insert_op = w::insert("users").row(doc().set("name", "Bob")).build();
    insert_via_tx(&repo, &table, &insert_op, false)
        .await
        .unwrap();

    // Update: add a brand-new field `email`.
    let update_op = w::update("users")
        .set(doc().set("email", "bob@example.com"))
        .returning(UpdateReturnMode::All)
        .build();
    let result = update_via_tx(&repo, &table, &update_op, &refs)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Bob".to_string()
        ))
    );
    assert_eq!(
        result.records[0].get_value_owned("email"),
        Some(shamir_types::types::value::QueryValue::Str(
            "bob@example.com".to_string()
        ))
    );
}
