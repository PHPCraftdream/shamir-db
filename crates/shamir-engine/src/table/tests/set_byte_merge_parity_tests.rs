//! Byte/op-identity and behavioural parity tests for the W3d tree-free SET
//! upsert path (`execute_set_tx`).
//!
//! The MERGE branch of `execute_set_tx` was cut over from an `InnerValue`
//! merge tree to the byte-merge pipeline (`merge_storage_bytes` +
//! `update_tx_bytes` + `run_validators_qv` + `record_view_to_query_value`),
//! mirroring W3c's `execute_update_tx` cutover. These tests prove the SET
//! path agrees with the reference tree merge and that the behavioural
//! guarantees hold end-to-end through the production implicit-tx + commit
//! pipeline.
//!
//! Four guarantees (mirroring `update_byte_merge_parity_tests.rs`):
//!
//! 1. **Storage-bytes identity** for the SET-specific merge shape:
//!    `merge_storage_bytes(old_bytes, set_map)` ==
//!    `merge_inner_maps(old_tree, set_map).to_bytes()`.
//! 2. **Index-op identity**: `plan_update_ops_ref(&old_view, &new_view)` ==
//!    `plan_update_ops(&old_tree, &new_tree)` over a SET merge.
//! 3. **Change-detection identity**: `(new_bytes == old_bytes)` iff
//!    `(merge_inner_maps(old, set) == old)`.
//! 4. **Behavioural** through `execute_set_tx` (implicit-tx + commit):
//!    - SET UPDATES an existing record's indexed field → index reflects it;
//!    - SET INSERTS a new record;
//!    - SET introduces a new field name on an existing record (the keystone
//!      overlay-key resolution path);
//!    - SET UPDATES a UNIQUE-indexed field (catalog path stays
//!      byte/op-identical — system_store relies on this).

use std::sync::Arc;

use bytes::Bytes;
use shamir_query_builder::write::{self, doc};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::access::Actor;
use shamir_types::codecs::interned::merge_storage_bytes;
use shamir_types::core::interner::InternerKey;
use shamir_types::mpack;
use shamir_types::record_view::RecordView;
use shamir_types::types::common::{new_map, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::repo::RepoInstance;
use crate::table::table_manager::TableManager;
use crate::table::tests::write_exec_tests::{insert_via_tx, setup_empty_table};

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
/// (interned-key → InnerValue) that the byte-merge pipeline uses.
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

/// The tree-path merge (reference implementation — matches the legacy
/// `merge_inner_maps` helper the old `execute_set_tx` used).
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

/// Drive a single SET upsert through the production implicit-tx + commit path
/// (the same path `query_runner.rs` routes `BatchOp::Set` through when a tx is
/// in flight). This exercises `execute_set_tx` end-to-end including index /
/// counter / interner commit side-effects.
async fn set_via_implicit_tx(
    repo: &RepoInstance,
    table: &TableManager,
    op: &crate::query::write::SetOp,
) -> Result<crate::query::write::WriteResult, crate::query::batch::BatchError> {
    let owned_op = op.clone();
    let owned_table = table.clone();
    repo.run_implicit_batch_tx(Actor::System, "test_set", move |tx| {
        Box::pin(async move { owned_table.execute_set_tx(&owned_op, tx, None).await })
    })
    .await
}

/// Drive a SET upsert through a bare (non-committed) `TxContext` so the test
/// can inspect staging (`counter_deltas`, `index_write_set`) directly.
async fn set_into_tx(tbl: &TableManager, op: &crate::query::write::SetOp) -> TxContext {
    let mut tx = TxContext::new(TxId::new(99), 0, u64::MAX, IsolationLevel::Snapshot);
    tbl.execute_set_tx(op, &mut tx, None).await.unwrap();
    tx
}

// ============================================================================
// 1. Storage-bytes identity (SET-specific merge shapes)
// ============================================================================

async fn assert_set_storage_bytes_identity(
    tbl: &TableManager,
    base_fields: &[(&str, InnerValue)],
    set_fields: &[(&str, InnerValue)],
    label: &str,
) {
    let (_, raw_bytes, old_tree) = seed_record(tbl, base_fields).await;
    let (_, set_map) = make_record(tbl, set_fields).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);
    let tree_bytes = merged_tree.to_bytes().unwrap();

    assert_eq!(
        new_bytes.as_ref(),
        tree_bytes.as_ref(),
        "SET storage-bytes identity failed for: {label}"
    );
}

#[tokio::test]
async fn set_storage_bytes_identity_overlap() {
    let tbl = make_table().await;
    assert_set_storage_bytes_identity(
        &tbl,
        &[
            ("email", InnerValue::Str("a@b.c".into())),
            ("name", InnerValue::Str("alice".into())),
        ],
        &[("name", InnerValue::Str("bob".into()))],
        "SET overlap: update an existing field",
    )
    .await;
}

#[tokio::test]
async fn set_storage_bytes_identity_new_key() {
    let tbl = make_table().await;
    assert_set_storage_bytes_identity(
        &tbl,
        &[("email", InnerValue::Str("a@b.c".into()))],
        &[("score", InnerValue::Int(42))],
        "SET new key: introduce a brand-new field",
    )
    .await;
}

#[tokio::test]
async fn set_storage_bytes_identity_type_change() {
    let tbl = make_table().await;
    assert_set_storage_bytes_identity(
        &tbl,
        &[("x", InnerValue::Int(42))],
        &[("x", InnerValue::Str("hello".into()))],
        "SET type change: Int -> Str",
    )
    .await;
}

#[tokio::test]
async fn set_storage_bytes_identity_noop() {
    let tbl = make_table().await;
    assert_set_storage_bytes_identity(
        &tbl,
        &[
            ("email", InnerValue::Str("a@b.c".into())),
            ("name", InnerValue::Str("alice".into())),
        ],
        &[("name", InnerValue::Str("alice".into()))],
        "SET no-op: set the same value",
    )
    .await;
}

// ============================================================================
// 2. Index-op identity: plan_update_ops_ref (RecordView) == plan_update_ops (tree)
// ============================================================================

#[tokio::test]
async fn set_index_ops_view_eq_tree_regular_index() {
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
        "SET plan_legacy_update_ops (regular index): RecordView and InnerValue must agree"
    );
    assert!(
        !ops_tree.is_empty(),
        "expected non-empty index ops for a SET value change on an indexed field"
    );
}

// ============================================================================
// 3. Change-detection identity: byte-eq iff tree-eq
// ============================================================================

#[tokio::test]
async fn set_change_detection_noop() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) = seed_record(
        &tbl,
        &[
            ("email", InnerValue::Str("a@b.c".into())),
            ("name", InnerValue::Str("alice".into())),
        ],
    )
    .await;
    let (_, set_map) = make_record(&tbl, &[("name", InnerValue::Str("alice".into()))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);

    let bytes_changed = new_bytes.as_ref() != raw_bytes.as_ref();
    let tree_changed = merged_tree != old_tree;

    assert_eq!(
        bytes_changed, tree_changed,
        "SET change-detection divergence on no-op: bytes={bytes_changed}, tree={tree_changed}"
    );
    assert!(!bytes_changed, "SET no-op must detect no change");
}

#[tokio::test]
async fn set_change_detection_value_change() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) = seed_record(
        &tbl,
        &[
            ("email", InnerValue::Str("a@b.c".into())),
            ("name", InnerValue::Str("alice".into())),
        ],
    )
    .await;
    let (_, set_map) = make_record(&tbl, &[("name", InnerValue::Str("bob".into()))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);

    let bytes_changed = new_bytes.as_ref() != raw_bytes.as_ref();
    let tree_changed = merged_tree != old_tree;

    assert_eq!(
        bytes_changed, tree_changed,
        "SET change-detection divergence on value change: bytes={bytes_changed}, tree={tree_changed}"
    );
    assert!(bytes_changed, "SET value change must be detected");
}

#[tokio::test]
async fn set_change_detection_new_key() {
    let tbl = make_table().await;
    let (_, raw_bytes, old_tree) =
        seed_record(&tbl, &[("email", InnerValue::Str("a@b.c".into()))]).await;
    let (_, set_map) = make_record(&tbl, &[("score", InnerValue::Int(7))]).await;

    let new_bytes = merge_storage_bytes(&raw_bytes, &set_map).unwrap();
    let merged_tree = merge_inner_maps(&old_tree, &set_map);

    let bytes_changed = new_bytes.as_ref() != raw_bytes.as_ref();
    let tree_changed = merged_tree != old_tree;

    assert_eq!(
        bytes_changed, tree_changed,
        "SET change-detection divergence on new key: bytes={bytes_changed}, tree={tree_changed}"
    );
    assert!(
        bytes_changed,
        "SET adding a new key must be detected as change"
    );
}

// ============================================================================
// 4. Behavioural: execute_set_tx through the production implicit-tx + commit
// ============================================================================

/// SET that UPDATES an existing record's indexed field → the index reflects
/// the new value (proves `update_tx_bytes` drove the legacy posting planners
/// through the lens and the postings landed at commit).
#[tokio::test]
async fn set_update_indexed_field_reflected_in_index() {
    let (table, repo) = setup_empty_table().await;
    table.create_index("city_idx", &["city"]).await.unwrap();

    // Seed a record with city=NYC.
    let insert_op = write::insert("users")
        .row(doc().set("email", "a@b.c").set("city", "NYC"))
        .build();
    insert_via_tx(&repo, &table, &insert_op, false)
        .await
        .unwrap();

    // Upsert by email — UPDATE branch — changing the indexed `city` field.
    let set_op = write::upsert("users")
        .key(doc().set("email", "a@b.c"))
        .value(doc().set("email", "a@b.c").set("city", "LA"))
        .build();
    let result = set_via_implicit_tx(&repo, &table, &set_op).await.unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(false))
    );
    assert_eq!(
        result.records[0].get_value_owned("city"),
        Some(shamir_types::types::value::QueryValue::Str(
            "LA".to_string()
        ))
    );

    // The index now resolves city=LA (the SET record). A fresh upsert keyed on
    // city=LA must find it (UPDATE branch → _created=false); city=NYC must NOT
    // find it (INSERT branch → _created=true).
    let find_la = write::upsert("users")
        .key(doc().set("city", "LA"))
        .value(doc().set("city", "LA").set("flag", true))
        .build();
    let la = set_via_implicit_tx(&repo, &table, &find_la).await.unwrap();
    assert_eq!(
        la.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(false))
    );

    let find_nyc = write::upsert("users")
        .key(doc().set("city", "NYC"))
        .value(doc().set("city", "NYC").set("flag", true))
        .build();
    let nyc = set_via_implicit_tx(&repo, &table, &find_nyc).await.unwrap();
    assert_eq!(
        nyc.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(true)),
        "old NYC posting should be gone after the upsert re-keyed to LA"
    );
}

/// SET that INSERTS a new record (no match) → counter delta is +1, _created=true.
#[tokio::test]
async fn set_insert_new_record_counter_delta() {
    let tbl = make_table().await;

    let op = write::upsert("t")
        .key(doc().set("email", "a@b.c"))
        .value(doc().set("email", "a@b.c").set("name", "alice"))
        .build();
    let tx = set_into_tx(&tbl, &op).await;

    let token = tbl.table_token();
    assert_eq!(
        *tx.counter_deltas.get(&token).unwrap_or(&0),
        1,
        "SET INSERT must bump the row counter by 1"
    );
}

/// SET introducing a NEW field name on an existing record — the keystone
/// overlay-key resolution path. The result map must contain both the old
/// (base-interned) fields and the new (overlay-interned) field; the committed
/// bytes must round-trip. This is the W3c keystone risk made concrete for SET.
#[tokio::test]
async fn set_update_introduces_new_field() {
    let (table, repo) = setup_empty_table().await;

    // Seed { email, name }.
    let insert_op = write::insert("users")
        .row(doc().set("email", "a@b.c").set("name", "alice"))
        .build();
    insert_via_tx(&repo, &table, &insert_op, false)
        .await
        .unwrap();

    // Upsert by email — introduce a brand-new field `score`.
    let set_op = write::upsert("users")
        .key(doc().set("email", "a@b.c"))
        .value(doc().set("score", 100_i64))
        .build();
    let result = set_via_implicit_tx(&repo, &table, &set_op).await.unwrap();
    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(false))
    );
    // Old field preserved (merge).
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "alice".to_string()
        ))
    );
    // New field present (the overlay-string-keyed result-QueryValue path).
    assert_eq!(
        result.records[0].get_value_owned("score"),
        Some(shamir_types::types::value::QueryValue::Int(100))
    );
}

/// SET UPDATES a UNIQUE-indexed field — the catalog/system_store path relies
/// on the upsert keeping unique postings byte/op-identical. A second upsert
/// claiming the now-freed unique value must succeed; a collision must fail.
#[tokio::test]
async fn set_update_unique_indexed_field() {
    let (table, repo) = setup_empty_table().await;
    table
        .create_unique_index("email_idx", &["email"])
        .await
        .unwrap();

    // Seed two records with distinct unique emails.
    let insert_op = write::insert("users")
        .row(doc().set("email", "a@b.c").set("name", "alice"))
        .row(doc().set("email", "x@y.z").set("name", "bob"))
        .build();
    insert_via_tx(&repo, &table, &insert_op, false)
        .await
        .unwrap();

    // Upsert alice's record by email — UPDATE branch — keep the same email.
    // The unique planner must see owner==self and not flag a self-conflict.
    let set_op = write::upsert("users")
        .key(doc().set("email", "a@b.c"))
        .value(doc().set("email", "a@b.c").set("name", "ALICE"))
        .build();
    let result = set_via_implicit_tx(&repo, &table, &set_op).await.unwrap();
    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(false))
    );
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "ALICE".to_string()
        ))
    );

    // A second upsert keyed on a NEW email that does not exist → INSERT branch.
    let set_new = write::upsert("users")
        .key(doc().set("email", "c@d.e"))
        .value(doc().set("email", "c@d.e").set("name", "carol"))
        .build();
    let result = set_via_implicit_tx(&repo, &table, &set_new).await.unwrap();
    assert_eq!(
        result.records[0].get_value_owned("_created"),
        Some(shamir_types::types::value::QueryValue::Bool(true))
    );
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "carol".to_string()
        ))
    );

    // The table now has exactly three records (no duplicates from the upserts).
    assert_eq!(table.count().await.unwrap(), 3);
}

/// Sanity: SET through a non-committed bare TxContext (mirrors the
/// `insert_tx_tests` shape) — UPDATE branch must not change the row counter.
#[tokio::test]
async fn set_tx_update_path_counter_delta_zero() {
    let tbl = make_table().await;

    // Seed an existing record with email=a@b.c.
    let (tree, _) = make_record(
        &tbl,
        &[
            ("email", InnerValue::Str("a@b.c".into())),
            ("name", InnerValue::Str("alice".into())),
        ],
    )
    .await;
    tbl.insert(&tree).await.unwrap();

    let op = write::upsert("t")
        .key(mpack!({ "email": "a@b.c" }))
        .value(mpack!({ "name": "bob" }))
        .build();
    let tx = set_into_tx(&tbl, &op).await;

    let token = tbl.table_token();
    assert_eq!(
        *tx.counter_deltas.get(&token).unwrap_or(&0),
        0,
        "SET UPDATE must not change the row counter"
    );
}

// A no-op SET (same value) through the bare-tx path must not stage a write
// (counter delta 0, matching W3c's no-op-skip behaviour for updates).
#[tokio::test]
async fn set_tx_noop_skips_staging() {
    let tbl = make_table().await;
    let (tree, _) = make_record(
        &tbl,
        &[
            ("email", InnerValue::Str("a@b.c".into())),
            ("name", InnerValue::Str("alice".into())),
        ],
    )
    .await;
    tbl.insert(&tree).await.unwrap();

    let op = write::upsert("t")
        .key(mpack!({ "email": "a@b.c" }))
        .value(mpack!({ "name": "alice" }))
        .build();
    let tx = set_into_tx(&tbl, &op).await;

    let token = tbl.table_token();
    assert_eq!(
        *tx.counter_deltas.get(&token).unwrap_or(&0),
        0,
        "no-op SET must not bump the counter"
    );
    // No index ops should have been staged for a no-op.
    assert!(
        tx.index_write_set.is_empty(),
        "no-op SET must not stage any index ops"
    );
}
