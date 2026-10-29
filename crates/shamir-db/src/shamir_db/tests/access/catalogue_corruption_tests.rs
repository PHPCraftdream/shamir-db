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
use futures::StreamExt;
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

async fn corrupt_persisted_row(table: &TableManager, table_name: &str, field: &str, value: &str) {
    let refs = new_map();
    let ctx = table
        .info_store_read_view()
        .filter_context(&refs)
        .await
        .unwrap();
    let result = table.read(&ReadQuery::new(table_name), &ctx).await.unwrap();
    assert!(result.corrupt_records.is_empty());
    let mut ids = Vec::new();
    let stream = table.list_stream(1000);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (id, row) in batch.unwrap() {
            let inner = row.into_inner().unwrap();
            let decoded = table
                .info_store_read_view()
                .inner_to_query_value(&inner)
                .await
                .unwrap();
            if decoded.get(field).and_then(QueryValue::as_str) == Some(value) {
                ids.push(id);
            }
        }
    }
    assert_eq!(ids.len(), 1);
    let id = ids[0];
    table
        .mvcc_store_for_test()
        .unwrap()
        .set_versioned(
            RecordKey::from_slice(&id.to_bytes()),
            Bytes::from_static(NOT_A_MAP),
        )
        .await
        .unwrap();
    let result = table.read(&ReadQuery::new(table_name), &ctx).await.unwrap();
    assert_eq!(result.corrupt_records.len(), 1);
    assert_eq!(result.corrupt_records[0].id, id);
}

#[tokio::test]
async fn corrupt_setting_row_denies_authorization() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let path = ResourcePath::FunctionNamespace;
    let actor = Actor::User(99);
    assert!(shamir
        .system_store()
        .load_setting("fn_namespace_meta")
        .await
        .unwrap()
        .is_none());
    assert_eq!(shamir.resource_meta(&path).await.unwrap().mode, 0o777);
    assert!(shamir
        .authorize_access(&actor, &path, Action::Read)
        .await
        .is_ok());
    let meta = ResourceMeta {
        owner: Actor::User(41),
        group: None,
        mode: 0o700,
    };
    shamir.set_resource_meta(&path, &meta).await.unwrap();
    let value = shamir
        .system_store()
        .load_setting("fn_namespace_meta")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ResourceMeta::from_record(&value).mode, 0o700);
    assert!(shamir
        .authorize_access(&actor, &path, Action::Read)
        .await
        .is_err());
    let table = shamir
        .system_store()
        .system_repo()
        .unwrap()
        .get_table("settings")
        .await
        .unwrap();
    corrupt_persisted_row(&table, "settings", "key", "fn_namespace_meta").await;
    let error = shamir
        .system_store()
        .load_setting("fn_namespace_meta")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("fn_namespace_meta"));
    assert!(shamir.resource_meta(&path).await.is_err());
    assert!(shamir
        .authorize_access(&actor, &path, Action::Read)
        .await
        .is_err());
}

#[tokio::test]
async fn corrupt_explicit_folder_denies_authorization() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let path = ResourcePath::function_folder(vec!["reports".to_string()]);
    let actor = Actor::User(99);
    assert!(shamir
        .system_store()
        .load_function_folder("reports")
        .await
        .unwrap()
        .is_none());
    assert_eq!(shamir.resource_meta(&path).await.unwrap().mode, 0o777);
    assert!(shamir
        .authorize_access(&actor, &path, Action::Read)
        .await
        .is_ok());
    shamir
        .create_function_folder_as(&["reports".to_string()], Actor::User(41))
        .await
        .unwrap();
    let meta = ResourceMeta {
        owner: Actor::User(41),
        group: None,
        mode: 0o700,
    };
    shamir.set_resource_meta(&path, &meta).await.unwrap();
    let row = shamir
        .system_store()
        .load_function_folder("reports")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get("path").and_then(QueryValue::as_str),
        Some("reports")
    );
    assert_eq!(ResourceMeta::from_record(&row).mode, 0o700);
    assert!(shamir
        .authorize_access(&actor, &path, Action::Read)
        .await
        .is_err());
    let table = shamir
        .system_store()
        .system_repo()
        .unwrap()
        .get_table("function_folders")
        .await
        .unwrap();
    corrupt_persisted_row(&table, "function_folders", "path", "reports").await;
    let error = shamir
        .system_store()
        .load_function_folder("reports")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("reports"));
    assert!(shamir.system_store().load_function_folders().await.is_err());
    assert!(shamir.resource_meta(&path).await.is_err());
    assert!(shamir
        .authorize_access(&actor, &path, Action::Read)
        .await
        .is_err());
}

#[tokio::test]
async fn corrupt_group_catalogue_denies_access_and_invocation() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let gid = shamir.create_group("readers").await.unwrap();
    shamir.add_group_member(gid, 99).await.unwrap();
    shamir
        .register_fn("grouped", false, |_, _, _| async { Ok(QueryValue::Null) })
        .unwrap();
    let path = ResourcePath::function("grouped");
    let meta = |mode| ResourceMeta {
        owner: Actor::User(41),
        group: Some(gid),
        mode,
    };
    let mut fields = new_map();
    fields.insert("name".to_string(), QueryValue::Str("grouped".to_string()));
    let record = QueryValue::Map(fields);
    shamir
        .system_store()
        .save_function("grouped", &record, &meta(0o740))
        .await
        .unwrap();
    let persisted = shamir
        .system_store()
        .load_function("grouped")
        .await
        .unwrap()
        .unwrap();
    let persisted_meta = ResourceMeta::from_record(&persisted);
    assert_eq!(persisted_meta.group, Some(gid));
    assert_eq!(persisted_meta.mode, 0o740);
    assert!(shamir
        .authorize_access(&Actor::User(99), &path, Action::Read)
        .await
        .is_ok());
    shamir
        .system_store()
        .save_function("grouped", &record, &meta(0o704))
        .await
        .unwrap();
    let persisted = shamir
        .system_store()
        .load_function("grouped")
        .await
        .unwrap()
        .unwrap();
    let persisted_meta = ResourceMeta::from_record(&persisted);
    assert_eq!(persisted_meta.group, Some(gid));
    assert_eq!(persisted_meta.mode, 0o704);
    assert!(shamir
        .authorize_access(&Actor::User(99), &path, Action::Read)
        .await
        .is_err());
    assert!(shamir
        .authorize_access(&Actor::User(100), &path, Action::Read)
        .await
        .is_ok());
    shamir
        .system_store()
        .save_function("grouped", &record, &meta(0o705))
        .await
        .unwrap();
    let persisted = shamir
        .system_store()
        .load_function("grouped")
        .await
        .unwrap()
        .unwrap();
    let persisted_meta = ResourceMeta::from_record(&persisted);
    assert_eq!(persisted_meta.group, Some(gid));
    assert_eq!(persisted_meta.mode, 0o705);
    assert!(shamir
        .invoke_function_as(
            "grouped",
            crate::engine::function::Params::new(),
            Actor::User(99),
        )
        .await
        .is_err());
    assert!(shamir
        .invoke_function_as(
            "grouped",
            crate::engine::function::Params::new(),
            Actor::User(100)
        )
        .await
        .is_ok());
    let table = stage_corrupt_row(&shamir, "groups").await;
    assert_corrupt_record_reported(&table, "groups").await;
    let error = shamir.system_store().load_group(gid).await.unwrap_err();
    assert!(error.to_string().contains(&format!("group {gid}")));
    assert!(shamir.system_store().load_groups().await.is_err());
    assert!(shamir
        .authorize_access(&Actor::User(100), &path, Action::Read)
        .await
        .is_err());
    assert!(shamir
        .invoke_function_as(
            "grouped",
            crate::engine::function::Params::new(),
            Actor::User(100)
        )
        .await
        .is_err());
    shamir
        .system_store()
        .save_function("grouped", &record, &meta(0o704))
        .await
        .unwrap();
    assert!(shamir
        .authorize_access(&Actor::User(100), &path, Action::Read)
        .await
        .is_err());
}

#[tokio::test]
async fn invocation_denies_settings_storage_read_failure() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir
        .register_fn("sample", false, |_, _, _| async { Ok(QueryValue::Null) })
        .unwrap();
    let fault = std::sync::Arc::new(super::access_meta_tests::failing_store::FailingStore::new());
    let repo = shamir.system_store().system_repo().unwrap();
    let table = TableManager::create_with_domain(
        "settings".to_string(),
        fault.clone() as std::sync::Arc<dyn shamir_storage::types::Store>,
        std::sync::Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new()),
        std::sync::Arc::clone(repo.write_domain()),
    )
    .await
    .unwrap()
    .with_interner(repo.repo_interner().await.unwrap());
    repo.install_table_for_test("settings", table);
    assert!(shamir
        .invoke_function_as(
            "sample",
            crate::engine::function::Params::new(),
            Actor::User(99)
        )
        .await
        .is_ok());
    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(shamir
        .system_store()
        .load_invocation_settings()
        .await
        .is_err());
    assert!(shamir
        .invoke_function_as(
            "sample",
            crate::engine::function::Params::new(),
            Actor::User(99)
        )
        .await
        .is_err());
}

#[tokio::test]
async fn invocation_denies_corrupt_settings_catalogue() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir
        .register_fn("sample", false, |_, _, _| async { Ok(QueryValue::Null) })
        .unwrap();
    let table = stage_corrupt_row(&shamir, "settings").await;
    assert_corrupt_record_reported(&table, "settings").await;
    assert!(shamir
        .system_store()
        .load_invocation_settings()
        .await
        .is_err());
    assert!(shamir
        .invoke_function_as(
            "sample",
            crate::engine::function::Params::new(),
            Actor::User(99),
        )
        .await
        .is_err());
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
