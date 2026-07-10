//! Op-parity and behavioural tests for the RecordView delete path.
//!
//! Two guarantees:
//!
//! 1. **Index-op identity**: `plan_delete_ops` and `plan_legacy_delete_ops`
//!    called with a `RecordView` built from the storage bytes emit the
//!    byte-identical `IndexWriteOp` set as those same planners called with
//!    `InnerValue::from_bytes(same_bytes)`. This proves the lens delete path
//!    cannot silently diverge from the tree path.
//!
//! 2. **Behavioural**: deleting a record that has a regular + unique +
//!    nested-path index removes ALL associated index entries. The committed
//!    index is consulted after the delete to prove the entries are gone.

use std::sync::Arc;

use bytes::Bytes;
use shamir_query_builder::{filter, write};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, Store};
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::tests::write_exec_tests::{delete_via_tx, setup_empty_table};
use crate::table::TableManager;

// ============================================================================
// Helpers
// ============================================================================

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

/// Seed a record with indexed fields:
///   - `city`      (plain string, targeted by a regular index in some tests)
///   - `email`     (targeted by a unique index in some tests)
///   - `addr.city` (nested path, targeted by a nested index in some tests)
///   - `score`     (int, unindexed filler)
///
/// Interns field names, inserts, and returns `(rid, raw_storage_bytes)`.
async fn seed_indexed_record(tbl: &TableManager) -> (RecordId, Bytes) {
    let interner = tbl.interner().get().await.unwrap();
    let city_k = interner.touch_ind("city").unwrap().into_key();
    let email_k = interner.touch_ind("email").unwrap().into_key();
    let addr_k = interner.touch_ind("addr").unwrap().into_key();
    let score_k = interner.touch_ind("score").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut addr_map = new_map();
    addr_map.insert(city_k.clone(), InnerValue::Str("NYC".into()));

    let mut m = new_map();
    m.insert(city_k.clone(), InnerValue::Str("NYC".into()));
    m.insert(email_k.clone(), InnerValue::Str("alice@example.com".into()));
    m.insert(addr_k.clone(), InnerValue::Map(addr_map));
    m.insert(score_k.clone(), InnerValue::Int(42));

    let rid = tbl.insert(&InnerValue::Map(m)).await.unwrap();
    let raw_bytes = tbl.data_store().get(rid.to_bytes().into()).await.unwrap();
    (rid, raw_bytes)
}

// ============================================================================
// IndexWriteOp comparison helpers
// ============================================================================

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
// 1. plan_delete_ops identity — RecordView vs InnerValue (index2 backends)
// ============================================================================

/// No index2 backends → both paths return empty.
/// Proves the generic dispatch compiles and doesn't panic.
#[tokio::test]
async fn plan_delete_ops_view_eq_tree_no_backends() {
    let tbl = make_table().await;
    let (rid, raw_bytes) = seed_indexed_record(&tbl).await;

    let old_tree = InnerValue::from_bytes(&raw_bytes).unwrap();
    let old_view = RecordView::new(&raw_bytes).unwrap();
    let tx_id = Some(shamir_tx::TxId::new(1));

    let mut ops_tree = tbl.plan_delete_ops(rid, &old_tree, tx_id).await.unwrap();
    let mut ops_view = tbl.plan_delete_ops(rid, &old_view, tx_id).await.unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);

    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_delete_ops: RecordView and InnerValue paths must produce identical IndexWriteOps"
    );
}

// ============================================================================
// 2. plan_legacy_delete_ops identity — RecordView vs InnerValue
// ============================================================================

/// No legacy indexes → both paths return empty.
#[tokio::test]
async fn plan_legacy_delete_ops_view_eq_tree_no_index() {
    let tbl = make_table().await;
    let (rid, raw_bytes) = seed_indexed_record(&tbl).await;

    let old_tree = InnerValue::from_bytes(&raw_bytes).unwrap();
    let old_view = RecordView::new(&raw_bytes).unwrap();

    let mut ops_tree = tbl.plan_legacy_delete_ops(rid, &old_tree).await.unwrap();
    let mut ops_view = tbl.plan_legacy_delete_ops(rid, &old_view).await.unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);

    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_legacy_delete_ops (no index): RecordView and InnerValue paths must agree"
    );
}

/// Regular index on `city` → both paths emit a RemovePosting for the city entry.
#[tokio::test]
async fn plan_legacy_delete_ops_view_eq_tree_regular_index() {
    let tbl = make_table().await;
    tbl.create_index("city_idx", &["city"]).await.unwrap();

    let (rid, raw_bytes) = seed_indexed_record(&tbl).await;

    let old_tree = InnerValue::from_bytes(&raw_bytes).unwrap();
    let old_view = RecordView::new(&raw_bytes).unwrap();

    let mut ops_tree = tbl.plan_legacy_delete_ops(rid, &old_tree).await.unwrap();
    let mut ops_view = tbl.plan_legacy_delete_ops(rid, &old_view).await.unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);

    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_legacy_delete_ops (regular index): RecordView and InnerValue paths must produce identical RemovePosting ops"
    );

    // Sanity: at least one RemovePosting was emitted (non-vacuous).
    assert!(
        !ops_view.is_empty(),
        "expected at least one RemovePosting from the city index"
    );
}

/// Regular index on `city` + unique index on `email` → ops must agree.
/// Expects at least two ops (one per index).
#[tokio::test]
async fn plan_legacy_delete_ops_view_eq_tree_unique_and_regular() {
    let tbl = make_table().await;
    tbl.create_index("city_idx", &["city"]).await.unwrap();
    tbl.create_unique_index("email_unique", &["email"])
        .await
        .unwrap();

    let (rid, raw_bytes) = seed_indexed_record(&tbl).await;

    let old_tree = InnerValue::from_bytes(&raw_bytes).unwrap();
    let old_view = RecordView::new(&raw_bytes).unwrap();

    let mut ops_tree = tbl.plan_legacy_delete_ops(rid, &old_tree).await.unwrap();
    let mut ops_view = tbl.plan_legacy_delete_ops(rid, &old_view).await.unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);

    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_legacy_delete_ops (unique+regular): RecordView and InnerValue must produce identical ops"
    );
    assert!(
        ops_view.len() >= 2,
        "expected at least 2 ops (regular + unique), got {}",
        ops_view.len()
    );
}

/// Nested-path index on `addr.city` → ops must agree.
#[tokio::test]
async fn plan_legacy_delete_ops_view_eq_tree_nested_path() {
    let tbl = make_table().await;
    tbl.create_index("addr_city_idx", &["addr.city"])
        .await
        .unwrap();

    let (rid, raw_bytes) = seed_indexed_record(&tbl).await;

    let old_tree = InnerValue::from_bytes(&raw_bytes).unwrap();
    let old_view = RecordView::new(&raw_bytes).unwrap();

    let mut ops_tree = tbl.plan_legacy_delete_ops(rid, &old_tree).await.unwrap();
    let mut ops_view = tbl.plan_legacy_delete_ops(rid, &old_view).await.unwrap();

    sort_ops(&mut ops_tree);
    sort_ops(&mut ops_view);

    assert_eq!(
        ops_as_sortkeys(&ops_tree),
        ops_as_sortkeys(&ops_view),
        "plan_legacy_delete_ops (nested addr.city): RecordView and InnerValue must produce identical RemovePosting ops"
    );
    assert!(
        !ops_view.is_empty(),
        "expected at least one RemovePosting from the nested addr.city index"
    );
}

// ============================================================================
// 3. Behavioural: execute_delete_tx removes regular + unique index entries
// ============================================================================

/// Insert a record into a table with a regular index on `city` and a unique
/// index on `email`. Delete via the production `execute_delete_tx` path
/// (wrapped in an implicit tx by `delete_via_tx`). Assert both index entries
/// are gone after commit and that the unique slot is freed for reuse.
#[tokio::test]
async fn execute_delete_tx_removes_regular_and_unique_index_entries() {
    let (table, repo) = setup_empty_table().await;

    // Add indexes before inserting so they are back-filled on create.
    table.create_index("city_idx", &["city"]).await.unwrap();
    table
        .create_unique_index("email_unique", &["email"])
        .await
        .unwrap();

    // Insert one record through the production path.
    let interner = table.interner().get().await.unwrap();
    let city_k = interner.touch_ind("city").unwrap().into_key();
    let email_k = interner.touch_ind("email").unwrap().into_key();
    table.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(city_k.clone(), InnerValue::Str("NYC".into()));
    m.insert(email_k.clone(), InnerValue::Str("alice@example.com".into()));
    table.insert(&InnerValue::Map(m.clone())).await.unwrap();

    // Sanity: city index has one entry before delete.
    let city_before = table
        .lookup_by_index("city_idx", &[InnerValue::Str("NYC".into())])
        .await
        .unwrap();
    assert_eq!(
        city_before.len(),
        1,
        "city index must have 1 entry before delete"
    );

    // Delete via the production implicit-tx path (migrated RecordView lens).
    let op = write::delete("users")
        .where_(filter::eq("city", "NYC"))
        .build();
    let refs = new_map();
    delete_via_tx(&repo, &table, &op, &refs).await.unwrap();

    // Record is gone.
    assert_eq!(
        table.count().await.unwrap(),
        0,
        "table must be empty after delete"
    );

    // Regular index entry removed.
    let city_after = table
        .lookup_by_index("city_idx", &[InnerValue::Str("NYC".into())])
        .await
        .unwrap();
    assert_eq!(
        city_after.len(),
        0,
        "city index must be empty after delete (RecordView lens path must remove posting)"
    );

    // Unique index slot freed: re-insert must succeed (no uniqueness violation).
    // If the unique slot was not freed this would return Err(UniqueViolation).
    table.insert(&InnerValue::Map(m)).await.unwrap_or_else(|e| {
        panic!("re-insert after delete failed — unique slot not freed: {e}");
    });
    assert_eq!(table.count().await.unwrap(), 1);
}

// ============================================================================
// 4. Behavioural: delete_tx (low-level) stages correct index ops
// ============================================================================

/// Use `delete_tx` directly with a manual `TxContext` to verify that the
/// migrated RecordView path stages `RemovePosting` ops corresponding to a
/// regular index. Commit the staged ops manually, then confirm the index is
/// cleared.
#[tokio::test]
async fn delete_tx_stages_remove_posting_for_legacy_index() {
    let tbl = make_table().await;
    tbl.create_index("city_idx", &["city"]).await.unwrap();

    let (rid, _) = seed_indexed_record(&tbl).await;

    // Verify index entry exists before delete.
    let before = tbl
        .lookup_by_index("city_idx", &[InnerValue::Str("NYC".into())])
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        1,
        "city index must have 1 entry before delete"
    );

    // Stage the delete via the migrated delete_tx.
    let mut tx = TxContext::new(TxId::new(10), 0, u64::MAX, IsolationLevel::Snapshot);
    let deleted = tbl.delete_tx(rid, Some(&mut tx)).await.unwrap();
    assert!(deleted, "delete_tx must report the record was found");

    // At least one RemovePosting must be staged for the city index.
    let rm_count = tx
        .index_write_set
        .iter()
        .filter(|(_, op)| matches!(op, shamir_tx::IndexWriteOp::RemovePosting { .. }))
        .count();
    assert!(
        rm_count >= 1,
        "expected at least one staged RemovePosting, got 0"
    );

    // Manually commit data ops.
    let token = tbl.table_token();
    if let Some(staging) = tx.write_set.remove(&token) {
        for op in staging.drain() {
            match op {
                KvOp::Set(key, v) => {
                    tbl.data_store().set(key, v).await.unwrap();
                }
                KvOp::Remove(key) => {
                    let _ = tbl.data_store().remove(key).await;
                }
            }
        }
    }

    // Commit index ops + invalidate posting cache (mirrors commit_tx
    // Phase 5c which calls apply_index_ops_at_commit + invalidate).
    let index_ops: Vec<shamir_tx::IndexWriteOp> = tx
        .index_write_set
        .iter()
        .map(|(_, op)| op.clone())
        .collect();
    for (_, op) in tx.index_write_set {
        match op {
            shamir_tx::IndexWriteOp::SetPosting { key, value } => {
                tbl.info_store().set(key.into(), value).await.unwrap();
            }
            shamir_tx::IndexWriteOp::RemovePosting { key } => {
                let _ = tbl.info_store().remove(key.into()).await;
            }
            shamir_tx::IndexWriteOp::BumpFtsStats { .. } => {}
        }
    }
    tbl.index_manager()
        .invalidate_posting_cache_for_ops(&index_ops);

    // Index entry is now gone.
    let after = tbl
        .lookup_by_index("city_idx", &[InnerValue::Str("NYC".into())])
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        0,
        "city index must be empty after committing the staged delete ops"
    );
}
