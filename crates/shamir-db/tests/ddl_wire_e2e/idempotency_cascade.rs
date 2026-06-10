//! if_not_exists, cascade, and referential integrity tests.

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::WriteOp;

use super::helpers::*;

// ═══════════════════════════════════════════════════════════════════════
// Phase 1a: create_table + insert in one non-tx batch via `after`
// ═══════════════════════════════════════════════════════════════════════

/// A single non-transactional batch that creates a table and inserts
/// rows into it, using the `after` ordering edge to guarantee the DDL
/// executes before the DML.
///
/// NOTE: the transactional variant is intentionally omitted — admin ops
/// are not tx-aware (they bypass the MVCC pipeline), so a transactional
/// batch mixing DDL and DML would require a separate design effort.
#[tokio::test]
async fn create_table_then_insert_via_after_non_tx() {
    // Setup: db + repo (no "items" table yet).
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    db.add_repo("testdb", repo_config).await.unwrap();

    // One batch: create_table("items") + insert into "items", with
    // `after: ["mk"]` on the insert.
    let mut b = Batch::new();
    b.id("phase1a");
    let mk = b.create_table("mk", ddl::create_table("items").repo("main"));
    let rows = b.insert(
        "rows",
        shamir_query_builder::write::insert("items")
            .row(shamir_query_builder::doc! { "name" => "Widget", "qty" => 10 })
            .row(shamir_query_builder::doc! { "name" => "Gadget", "qty" => 5 }),
    );
    b.after(&rows, &mk);
    let req = b.to_request_via_msgpack();

    let resp = db
        .execute("testdb", &req)
        .await
        .expect("batch with create_table + insert via after should succeed");

    // The execution plan must have two stages: mk first, rows second.
    assert_eq!(
        resp.execution_plan.len(),
        2,
        "expected 2 stages (DDL then DML), got: {:?}",
        resp.execution_plan
    );
    assert_eq!(resp.execution_plan[0], vec!["mk"]);
    assert_eq!(resp.execution_plan[1], vec!["rows"]);

    // Verify insert actually landed: read back the rows.
    let mut b = Batch::new();
    b.id("verify");
    b.query("q", shamir_query_builder::q!(from items));
    let read_req = b.to_request_via_msgpack();
    let read_resp = db.execute("testdb", &read_req).await.unwrap();
    let records = &read_resp.results["q"].records;
    assert_eq!(records.len(), 2, "should have 2 inserted records");
}

// =====================================================================
// Phase 1b: idempotent create (if_not_exists)
// =====================================================================

#[tokio::test]
async fn create_table_duplicate_without_if_not_exists_fails() {
    let db = setup_db().await;

    // First create succeeds
    let mut b = Batch::new();
    b.id("ct1");
    b.create_table("op", ddl::create_table("orders").repo("main"));
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create without if_not_exists -> error
    let err = db.execute("testdb", &req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' error, got: {msg}"
    );
}

#[tokio::test]
async fn create_table_with_if_not_exists_idempotent() {
    let db = setup_db().await;

    // First create
    let mut b = Batch::new();
    b.id("ct");
    b.create_table(
        "op",
        ddl::create_table("orders").repo("main").if_not_exists(),
    );
    let req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create with if_not_exists -> OK, no error
    let resp = db.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], false);
    assert_eq!(resp.results["op"].records[0]["existed"], true);
}

#[tokio::test]
async fn create_db_with_if_not_exists_idempotent() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("bootstrap").await;

    let mut b = Batch::new();
    b.id("cd");
    b.create_db("op", ddl::create_db("newdb").if_not_exists());
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("bootstrap", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create with if_not_exists -> OK, existed=true
    let resp = shamir.execute("bootstrap", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], false);
    assert_eq!(resp.results["op"].records[0]["existed"], true);
}

#[tokio::test]
async fn create_repo_with_if_not_exists_idempotent() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let mut b = Batch::new();
    b.id("cr");
    b.create_repo(
        "op",
        ddl::create_repo("archive")
            .engine("in_memory")
            .if_not_exists(),
    );
    let req = b.to_request_via_msgpack();
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], true);
    assert_eq!(resp.results["op"].records[0]["existed"], false);

    // Second create -> OK
    let resp = shamir.execute("testdb", &req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["created"], false);
    assert_eq!(resp.results["op"].records[0]["existed"], true);
}

// =====================================================================
// Phase 1b: referential integrity on drop + cascade
// =====================================================================

#[tokio::test]
async fn drop_db_with_repos_no_cascade_fails() {
    let db = setup_db().await;

    // testdb has "main" repo -> drop without cascade should fail
    let mut b = Batch::new();
    b.id("dd");
    b.drop_db("op", ddl::drop_db("testdb"));
    let drop_req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &drop_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still has repositories"),
        "expected referential integrity error, got: {msg}"
    );
}

#[tokio::test]
async fn drop_db_with_cascade_succeeds() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("bootstrap").await;
    shamir.create_db("target_db").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("target_db", repo_config).await.unwrap();

    // Drop with cascade
    let mut b = Batch::new();
    b.id("dd");
    b.drop_db("op", ddl::drop_db("target_db").cascade());
    let drop_req = b.to_request_via_msgpack();
    let resp = shamir.execute("bootstrap", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Verify the db is gone
    assert!(!shamir.has_db("target_db"));
}

#[tokio::test]
async fn drop_repo_with_tables_no_cascade_fails() {
    let db = setup_db().await;

    // "main" repo has "users" table -> drop without cascade should fail
    let mut b = Batch::new();
    b.id("dr");
    b.drop_repo("op", ddl::drop_repo("main"));
    let drop_req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &drop_req).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("still has tables"),
        "expected referential integrity error, got: {msg}"
    );
}

#[tokio::test]
async fn drop_repo_with_cascade_succeeds() {
    let db = setup_db().await;

    // Drop "main" with cascade (it has "users" table)
    let mut b = Batch::new();
    b.id("dr");
    b.drop_repo("op", ddl::drop_repo("main").cascade());
    let drop_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Verify the repo is gone
    let db_inst = db.get_db("testdb").unwrap();
    assert!(db_inst.list_repos().is_empty());
}

// =====================================================================
// Phase 1b: drop_table cleans validator bound_in
// =====================================================================

#[tokio::test]
async fn drop_table_cleans_validator_bound_in() {
    let db = setup_db().await;

    // Step 1: create a validator and bind it to "users"
    let wasm = accept_wasm();
    let mut b = Batch::new();
    b.id("cv");
    b.create_validator(
        "op",
        ddl::create_validator("v_cleanup").wasm(wasm_b64(&wasm)),
    );
    let create_req = b.to_request_via_msgpack();
    db.execute("testdb", &create_req).await.unwrap();

    let mut b = Batch::new();
    b.id("bv");
    b.bind_validator(
        "op",
        ddl::bind_validator("v_cleanup", "users")
            .db("testdb")
            .ops([WriteOp::Insert])
            .priority(1500),
    );
    let bind_req = b.to_request_via_msgpack();
    db.execute("testdb", &bind_req).await.unwrap();

    // Step 2: drop the table -> should clean bound_in
    let mut b = Batch::new();
    b.id("dt");
    b.drop_table("op", ddl::drop_table("users").repo("main"));
    let drop_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &drop_req).await.unwrap();
    assert_eq!(resp.results["op"].records[0]["existed"], true);

    // Step 3: now drop_validator should succeed (bound_in was cleaned)
    let mut b = Batch::new();
    b.id("dv");
    b.drop_validator("op", ddl::drop_validator("v_cleanup"));
    let drop_val_req = b.to_request_via_msgpack();
    let resp = db.execute("testdb", &drop_val_req).await.unwrap();
    assert_eq!(
        resp.results["op"].records[0]["existed"], true,
        "validator should have existed and been dropped after bound_in cleanup"
    );
}
