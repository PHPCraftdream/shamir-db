//! ③.2d — server-stamping `created_at` / `updated_at` e2e tests.
//!
//! Exercises the full DDL execution path: create table, set schema with
//! `auto_now` / `auto_now_add` rules, insert a row and verify both fields are
//! stamped, then run an UPDATE and verify only `updated_at` changes while
//! `created_at` is preserved.
//!
//! ## Invariants under test
//!
//! * INSERT stamps both `created_at` (AutoNowAdd) and `updated_at` (AutoNow).
//! * UPDATE stamps ONLY `updated_at` (AutoNow). `created_at` is unchanged.
//! * AutoNow overwrites any caller-supplied value of `updated_at`.
//! * AutoNowAdd preserves an explicitly-supplied `created_at`.
//! * Both timestamps are stored as `Int` (nanoseconds since Unix epoch).

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{insert, update};
use shamir_query_builder::Query;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

async fn setup() -> (ShamirDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    db.create_db("testdb").await;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["events"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    (db, dir)
}

async fn exec_set_schema(db: &ShamirDb, rules: Vec<ddl::FieldBuilder>) -> Result<(), String> {
    let rules: Vec<_> = rules.into_iter().map(|b| b.build()).collect();
    let mut b = Batch::new();
    b.id(1);
    b.set_table_schema("ss", ddl::set_table_schema("events").rules(rules));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn try_insert_autocommit(db: &ShamirDb, record: QueryValue) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.insert("ins", insert("events").row(record));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn try_update_autocommit(
    db: &ShamirDb,
    filter_key: &str,
    filter_val: &str,
    set_map: QueryValue,
) -> Result<(), String> {
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        update("events")
            .where_(eq(filter_key, filter_val))
            .set(set_map),
    );
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn read_all(db: &ShamirDb) -> Vec<QueryValue> {
    let mut b = Batch::new();
    b.id(1);
    b.query("all", Query::from("events"));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .map_err(|e| e.to_string())
        .unwrap();
    resp.results["all"]
        .records
        .iter()
        .map(|r| r.as_value().into_owned())
        .collect()
}

fn get_int(rec: &QueryValue, field: &str) -> Option<i64> {
    match rec {
        QueryValue::Map(m) => match m.get(field) {
            Some(QueryValue::Int(i)) => Some(*i),
            _ => None,
        },
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: INSERT stamps both created_at and updated_at
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn insert_stamps_created_at_and_updated_at() {
    let (db, _dir) = setup().await;

    exec_set_schema(
        &db,
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["created_at"]).int().auto_now_add(),
            ddl::field(["updated_at"]).int().auto_now(),
        ],
    )
    .await
    .expect("set_table_schema should succeed");

    // Record before epoch 2020 (1577836800000000000 ns) so any server stamp > this.
    let before_insert_ns: i64 = 1_577_836_800_000_000_000;

    let ok = try_insert_autocommit(
        &db,
        mpack!({
            "name": "event-alpha"
        }),
    )
    .await;
    assert!(ok.is_ok(), "insert: {:?}", ok.err());

    let rows = read_all(&db).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("name"), Some(&mpack!("event-alpha")));

    let created = get_int(&rows[0], "created_at");
    let updated = get_int(&rows[0], "updated_at");

    assert!(
        created.is_some(),
        "INSERT must stamp created_at; got: {:?}",
        rows[0]
    );
    assert!(
        updated.is_some(),
        "INSERT must stamp updated_at; got: {:?}",
        rows[0]
    );
    assert!(
        created.unwrap() > before_insert_ns,
        "created_at should be after 2020-01-01: {}",
        created.unwrap()
    );
    assert!(
        updated.unwrap() > before_insert_ns,
        "updated_at should be after 2020-01-01: {}",
        updated.unwrap()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: UPDATE stamps only updated_at, preserves created_at
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn update_stamps_only_updated_at_preserves_created_at() {
    let (db, _dir) = setup().await;

    exec_set_schema(
        &db,
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["created_at"]).int().auto_now_add(),
            ddl::field(["updated_at"]).int().auto_now(),
        ],
    )
    .await
    .expect("set_table_schema should succeed");

    // Insert — both fields are stamped.
    let ok = try_insert_autocommit(
        &db,
        mpack!({
            "name": "event-beta"
        }),
    )
    .await;
    assert!(ok.is_ok(), "insert: {:?}", ok.err());

    let rows_after_insert = read_all(&db).await;
    assert_eq!(rows_after_insert.len(), 1);
    let created_at_after_insert =
        get_int(&rows_after_insert[0], "created_at").expect("created_at must be stamped on insert");
    let updated_at_after_insert =
        get_int(&rows_after_insert[0], "updated_at").expect("updated_at must be stamped on insert");

    // Brief sleep to ensure the clock advances, so updated_at will differ.
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;

    // UPDATE — rename; no created_at/updated_at supplied.
    let ok2 = try_update_autocommit(
        &db,
        "name",
        "event-beta",
        mpack!({
            "name": "event-beta-v2"
        }),
    )
    .await;
    assert!(ok2.is_ok(), "update: {:?}", ok2.err());

    let rows_after_update = read_all(&db).await;
    assert_eq!(rows_after_update.len(), 1);

    let created_at_after_update = get_int(&rows_after_update[0], "created_at")
        .expect("created_at must still be present after update");
    let updated_at_after_update = get_int(&rows_after_update[0], "updated_at")
        .expect("updated_at must be present after update");

    // created_at must NOT change on UPDATE.
    assert_eq!(
        created_at_after_insert, created_at_after_update,
        "created_at must be preserved by UPDATE (AutoNowAdd is insert-only)"
    );

    // updated_at MUST be >= the value after insert (server re-stamps on UPDATE).
    assert!(
        updated_at_after_update >= updated_at_after_insert,
        "updated_at after UPDATE ({}) must be >= value after INSERT ({})",
        updated_at_after_update,
        updated_at_after_insert
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: AutoNow overwrites a caller-supplied updated_at on INSERT
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn auto_now_overwrites_caller_supplied_updated_at() {
    let (db, _dir) = setup().await;

    exec_set_schema(
        &db,
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["updated_at"]).int().auto_now(),
        ],
    )
    .await
    .expect("set_table_schema should succeed");

    // Caller supplies an old epoch value for updated_at.
    let stale_ts: i64 = 1_000_000_000;
    let record = {
        let mut m = new_map();
        m.insert("name".to_string(), QueryValue::Str("event-gamma".into()));
        m.insert("updated_at".to_string(), QueryValue::Int(stale_ts));
        QueryValue::Map(m)
    };
    let ok = try_insert_autocommit(&db, record).await;
    assert!(ok.is_ok(), "insert: {:?}", ok.err());

    let rows = read_all(&db).await;
    assert_eq!(rows.len(), 1);
    let ts = get_int(&rows[0], "updated_at").expect("updated_at must be present");

    // AutoNow must overwrite the stale value with the server clock.
    assert!(
        ts > stale_ts,
        "AutoNow must overwrite stale caller-supplied updated_at; got {ts}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 4: AutoNowAdd preserves an explicit created_at on INSERT
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn auto_now_add_preserves_explicit_created_at() {
    let (db, _dir) = setup().await;

    exec_set_schema(
        &db,
        vec![
            ddl::field(["name"]).string().required(),
            ddl::field(["created_at"]).int().auto_now_add(),
        ],
    )
    .await
    .expect("set_table_schema should succeed");

    // Caller supplies an explicit created_at.
    let explicit_ts: i64 = 1_234_567_890_000_000_000;
    let record = {
        let mut m = new_map();
        m.insert("name".to_string(), QueryValue::Str("event-delta".into()));
        m.insert("created_at".to_string(), QueryValue::Int(explicit_ts));
        QueryValue::Map(m)
    };
    let ok = try_insert_autocommit(&db, record).await;
    assert!(ok.is_ok(), "insert: {:?}", ok.err());

    let rows = read_all(&db).await;
    assert_eq!(rows.len(), 1);
    let ts = get_int(&rows[0], "created_at").expect("created_at must be present");

    // AutoNowAdd must NOT overwrite the explicit value.
    assert_eq!(
        ts, explicit_ts,
        "AutoNowAdd must preserve explicit caller-supplied created_at"
    );
}
