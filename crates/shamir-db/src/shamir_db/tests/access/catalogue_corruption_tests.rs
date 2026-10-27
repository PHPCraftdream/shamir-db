//! RV-DB-DEC185C5 — corrupt catalogue rows must fail closed, at BOTH layers:
//! the read-level `corrupt_records` guard in `SystemStore` AND the
//! `checked_meta` non-Map rejection in `access::meta`.
//!
//! Injection mechanism (verified against the read path): MVCC-visible reads
//! pull record bytes from the per-table `MvccStore`, and
//! `QueryResult::corrupt_records` is appended exactly when
//! `RecordView::new(bytes)` fails during the scan (paged.rs / scan.rs). So
//! the corruption is written THROUGH the MVCC layer itself —
//! `MvccStore::set_versioned` (the same entry point the commit path uses,
//! keyed on `RecordId::to_bytes()`) staging a committed version whose body
//! is not a valid record map. The table's `__data__` store is inert under
//! MVCC, so writing there would be invisible — this writes where reads
//! actually come from. Each test first asserts the scan reports the corrupt
//! row, then asserts the catalogue loader propagates `Err` (never
//! `Ok(None)` → default-open meta).
//!
//! Layer note: `checked_meta` in `access::meta` is defense-in-depth — with
//! the read-level guard in place a non-Map record cannot normally reach it,
//! so its rejection is verified by code inspection, not end-to-end here.

use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::engine::table::TableManager;
use crate::query::read::ReadQuery;
use crate::shamir_db::ShamirDb;
use bytes::Bytes;
use shamir_storage::types::RecordKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

const NOT_A_MAP: &[u8] = b"not-a-map";

/// Stage one MVCC-committed row whose body is not a valid record map, then
/// return the table for read-back verification.
async fn stage_corrupt_row(shamir: &ShamirDb, table_name: &str) -> TableManager {
    let repo = shamir.system_store().system_repo().unwrap();
    let table = repo.get_table(table_name).await.unwrap();
    let mvcc = table
        .mvcc_store_for_test()
        .expect("system tables carry an attached MvccStore");
    let rid = RecordId::new();
    mvcc.set_versioned(
        RecordKey::from_slice(&rid.to_bytes()),
        Bytes::from_static(NOT_A_MAP),
    )
    .await
    .unwrap();
    table
}

/// Read `table_name` back through the engine and assert exactly one corrupt
/// record was reported — proving the malformed version reaches the
/// corruption accumulator rather than a backend decode error.
async fn assert_corrupt_record_reported(table: &TableManager, table_name: &str) {
    let refs = new_map();
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .unwrap();
    let result = table.read(&ReadQuery::new(table_name), &ctx).await.unwrap();
    assert_eq!(
        result.corrupt_records.len(),
        1,
        "the malformed MVCC version must surface as a corrupt record"
    );
    assert_eq!(result.corrupt_records[0].table, table_name);
}

#[tokio::test]
async fn rv_db_23a03103_corrupt_repository_row_still_fails_closed() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("db").await.unwrap();
    shamir
        .add_repo(
            "db",
            crate::engine::repo::RepoConfig::new(
                "repo",
                crate::engine::repo::BoxRepoFactory::in_memory(),
            ),
        )
        .await
        .unwrap();
    let store = shamir.system_store();
    let meta = ResourceMeta::open();
    store
        .save_repository("db", "repo", "memory", None, &meta, None)
        .await
        .unwrap();
    let table = stage_corrupt_row(&shamir, "repositories").await;
    assert_corrupt_record_reported(&table, "repositories").await;
    assert!(shamir
        .system_store()
        .load_repository_record("db", "repo")
        .await
        .is_err());
    let rename_error = shamir
        .rename_repo_as("db", "repo", "renamed", Actor::System)
        .await
        .unwrap_err();
    assert!(rename_error
        .to_string()
        .contains("corrupt repository catalogue"));
}

#[tokio::test]
async fn rv_db_dec185c5_corrupt_function_metadata_fails_closed() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let record = QueryValue::Map({
        let mut fields = new_map();
        fields.insert("name".to_string(), QueryValue::Str("damaged".to_string()));
        fields.insert("owner".to_string(), QueryValue::Int(41));
        fields.insert("group".to_string(), QueryValue::Null);
        fields.insert("mode".to_string(), QueryValue::Int(0o700));
        fields
    });
    shamir
        .system_store()
        .save_function(
            "damaged",
            &record,
            &ResourceMeta {
                owner: Actor::User(41),
                group: None,
                mode: 0o700,
            },
        )
        .await
        .unwrap();
    let table = stage_corrupt_row(&shamir, "functions").await;
    assert_corrupt_record_reported(&table, "functions").await;

    // Layer 1: the read-level guard must reject the scan as Err, never
    // collapse to Ok(None).
    assert!(shamir
        .system_store()
        .load_function("damaged")
        .await
        .is_err());
    // Default-open must be unreachable: User(99) holds no grants, so an Ok
    // here would mean the corrupt row collapsed to open meta.
    assert!(shamir
        .authorize_access(
            &Actor::User(99),
            &ResourcePath::function("damaged"),
            Action::Write,
        )
        .await
        .is_err());
}

#[tokio::test]
async fn rv_db_dec185c5_corrupt_database_metadata_fails_closed() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("damageddb").await.unwrap();
    let table = stage_corrupt_row(&shamir, "databases").await;
    assert_corrupt_record_reported(&table, "databases").await;

    assert!(shamir
        .system_store()
        .load_database("damageddb")
        .await
        .is_err());
    assert!(shamir
        .authorize_access(
            &Actor::User(99),
            &ResourcePath::database("damageddb"),
            Action::Write,
        )
        .await
        .is_err());
}
