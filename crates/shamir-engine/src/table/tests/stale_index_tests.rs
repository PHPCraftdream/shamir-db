//! Regression test: `lookup_records_via_index` silently skips stale index
//! entries (index postings pointing at record IDs absent from the table).
//!
//! On the **pre-fix** code the per-record `self.get(id).await?` call
//! propagated `NotFound` as an error. The fixed code uses `get_many`
//! and skips `None` entries with `let … else { continue }`.
//!
//! The test plants a stale posting via the low-level
//! `IndexManager::on_record_created` API (called with a fabricated
//! `RecordId` never inserted into the table) and then drives an
//! index-backed `execute_delete` whose WHERE clause matches the
//! indexed field+value, forcing `lookup_records_via_index` to encounter
//! both the real and the stale id.

#![allow(deprecated)]

use crate::db_instance::db_instance::DbInstance;
use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use shamir_query_builder::{filter, write};
use shamir_types::codecs::transform;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::UserValue;

/// Create an in-memory table, insert one real record with
/// `status = "active"`, create a regular index on `status`, plant a
/// stale posting (status="active" → fabricated RecordId), and return
/// the table manager together with the real record's id.
async fn setup_table_with_stale_index_entry() -> (crate::table::TableManager, RecordId) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();

    // --- Insert one real record with status="active" -----------------
    let interner = table.interner().get().await.unwrap();
    let mut map = new_map();
    map.insert("name".to_string(), UserValue::Str("Alice".into()));
    map.insert("status".to_string(), UserValue::Str("active".into()));
    let result = transform::user_to_inner(&UserValue::Map(map), interner);
    if let Some(ref new_keys) = result.new_keys {
        table.interner().save_new_keys(new_keys).await.unwrap();
    }
    let real_id = table.insert(&result.inner_value).await.unwrap();

    // --- Create a regular index on "status" --------------------------
    table.create_index("status_idx", &["status"]).await.unwrap();

    // --- Plant a stale posting: status="active" → fake_id ------------
    // Build an InnerValue::Map with the interned "status" key so that
    // `on_record_created` sees the field and writes a posting.
    let interner = table.interner().get().await.unwrap();
    let mut inner_map = new_map();
    let status_key = interner
        .get_ind("status")
        .expect("'status' must be interned after insert + index creation");
    inner_map.insert(
        status_key,
        shamir_types::types::value::InnerValue::Str("active".into()),
    );
    let fake_value = shamir_types::types::value::InnerValue::Map(inner_map);

    let fake_id = RecordId::new();
    // This writes a posting (status="active" → fake_id) into the index
    // store, but fake_id has no corresponding record in the data store.
    table
        .index_manager_ref()
        .on_record_created(&fake_id, &fake_value)
        .await
        .unwrap();

    (table, real_id)
}

/// Regression: `execute_delete` WHERE status="active" must succeed even
/// though the index contains a stale posting pointing at a non-existent
/// record.
///
/// Pre-fix behaviour: `lookup_records_via_index` called `self.get(id)`
/// per-record; the stale id caused a `NotFound` error → `Err(...)`.
///
/// Post-fix behaviour: `get_many` returns `None` for the stale id,
/// the `let Some(record) = record_opt else { continue }` guard skips
/// it, and only the real record is returned → `Ok(...)`.
#[tokio::test]
async fn test_stale_index_entry_skipped_in_lookup_records_via_index() {
    let (table, _real_id) = setup_table_with_stale_index_entry().await;

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Delete WHERE status = "active" — triggers index-backed path
    // because "status_idx" covers the Eq condition.
    let op = write::delete("users")
        .where_(filter::eq("status", "active"))
        .build();

    // This is the critical assertion: the operation must NOT error.
    // On the pre-fix code it would propagate NotFound for the stale id.
    let result = table.execute_delete(&op, &ctx).await.unwrap();

    // Only the one real record (Alice) should have been deleted.
    assert_eq!(
        result.affected, 1,
        "exactly one real record should be deleted"
    );

    // Table should now be empty.
    assert_eq!(
        table.count().await.unwrap(),
        0,
        "table should be empty after deleting the only real record"
    );
}
