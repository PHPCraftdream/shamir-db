//! P3 — nested sub-batch executor tests.
//!
//! Covers: bind resolution, param injection, tx-in-tx guard,
//! unbound_param error, and sub-batch atomicity.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, SubBatchOp,
};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::write::InsertOp;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

// ============================================================================
// Shared infrastructure
// ============================================================================

struct TestResolver {
    db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        // F4b-1: non-tx inserts route through an implicit Snapshot tx, so the
        // executor now resolves the repo even on the non-transactional path.
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }
}

async fn setup() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

struct TxTestResolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

// ============================================================================
// Test 1: sub_batch_runs_and_outer_reads_result
//
// Outer: a sub-batch inserts a row; an outer read sees it via $query @sub.
// ============================================================================

#[tokio::test]
async fn sub_batch_runs_and_outer_reads_result() {
    let resolver = setup().await;

    // Inner batch: insert one user.
    let mut inner_b = Batch::new();
    inner_b.id(10);
    inner_b.insert(
        "ins",
        write::insert("users").row(doc().set("name", "p3_alice").set("score", 99i64)),
    );
    let inner_req = inner_b.build();

    // Outer batch:
    //   sub  → BatchOp::Batch(inner_req)     — runs the inner batch
    //   read → reads users after the sub
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut read_queries = new_map();
    read_queries.insert("sub".to_string(), sub_entry);

    let read_q = shamir_query_builder::query::Query::from("users").where_eq("name", "p3_alice");
    read_queries.insert(
        "read".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(read_q)),
            return_result: true,
            after: vec!["sub".to_string()],
        },
    );

    let outer_req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: read_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // The outer read must see the row inserted by the inner batch.
    assert_eq!(
        resp.results["read"].records.len(),
        1,
        "outer read should see the row inserted by the sub-batch; got {:?}",
        resp.results["read"].records
    );
    assert_eq!(
        resp.results["read"].records[0].get_value_str("name"),
        Some("p3_alice")
    );

    // The sub-batch result should be present and have a `value` (the inner
    // results map serialised).
    assert!(
        resp.results["sub"].value.is_some(),
        "sub-batch result must carry a value field"
    );
}

// ============================================================================
// Test 2: sub_batch_bind_injects_param
//
// Outer: @user reads one user; sub-batch has bind: { uid: $query @user[0].score }
// Inner op uses $param uid in its WHERE clause.
// ============================================================================

#[tokio::test]
async fn sub_batch_bind_injects_param() {
    let resolver = setup().await;

    // Seed a user.
    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent(
        "seed",
        write::insert("users").row(doc().set("name", "param_alice").set("score", 42i64)),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Inner batch: read users where score == $param score_val.
    let inner_where = Filter::Eq {
        field: vec!["score".to_string()],
        value: FilterValue::Param {
            name: "score_val".to_string(),
        },
    };
    let inner_read = shamir_query_types::read::ReadQuery {
        from: crate::query::TableRef::new("users"),
        r#where: Some(inner_where),
        select: shamir_query_types::read::Select::all(),
        order_by: None,
        pagination: shamir_query_types::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: shamir_query_types::read::Temporal::default(),
        with_version: false,
    };

    let mut inner_queries = new_map();
    inner_queries.insert(
        "match".to_string(),
        QueryEntry {
            op: BatchOp::Read(inner_read),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(20),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Outer batch:
    //   user → read user with name param_alice (returns score=42)
    //   sub  → BatchOp::Batch(inner_req) with bind: { score_val: $query @user[0].score }
    let outer_read_q =
        shamir_query_builder::query::Query::from("users").where_eq("name", "param_alice");
    let outer_user_entry = QueryEntry {
        op: BatchOp::Read(crate::query::read::ReadQuery::from(outer_read_q)),
        return_result: true,
        after: Vec::new(),
    };

    let mut bind = new_map();
    bind.insert(
        "score_val".to_string(),
        FilterValue::QueryRef {
            alias: "@user".to_string(),
            path: Some("[0].score".to_string()),
        },
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("user".to_string(), outer_user_entry);
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(2),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // The sub result's value.match should contain the user with score 42.
    let sub_val = resp.results["sub"].value.as_ref().unwrap();
    let match_records = sub_val["match"]["records"]
        .as_array()
        .expect("match.records must be an array");
    assert_eq!(
        match_records.len(),
        1,
        "inner $param-filtered read should return exactly 1 record; got {:?}",
        match_records
    );
    assert_eq!(
        match_records[0]["score"], 42i64,
        "the returned record should have score=42"
    );
}

// ============================================================================
// Test 3: sub_batch_atomic
//
// A transactional sub-batch whose second op fails rolls back the first.
// ============================================================================

#[tokio::test]
async fn sub_batch_atomic() {
    use futures::StreamExt;

    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Inner batch: insert one row, then try to insert into a NON-EXISTENT table
    // so the second op fails and the tx rolls back.
    let mut inner_queries = new_map();
    inner_queries.insert(
        "good".to_string(),
        QueryEntry {
            op: BatchOp::Insert(shamir_query_types::write::InsertOp {
                insert_into: crate::query::TableRef::new("users"),
                values: vec![mpack!({ "name": "atomic_test" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    inner_queries.insert(
        "bad".to_string(),
        QueryEntry {
            op: BatchOp::Insert(shamir_query_types::write::InsertOp {
                insert_into: crate::query::TableRef::new("nonexistent_table"),
                values: vec![mpack!({ "name": "should_not_appear" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: vec!["good".to_string()],
        },
    );

    let inner_req = BatchRequest {
        id: QueryValue::Int(30),
        name: None,
        transactional: true, // atomic unit
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(3),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // The outer batch succeeds (the sub-batch failure is surfaced as an error).
    let result = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test").await;

    // The sub-batch should have failed (bad op → table not found before commit).
    // The outer batch wraps this as a QueryError.
    assert!(
        result.is_err(),
        "outer batch should propagate the sub-batch failure; got Ok"
    );

    // The table should be EMPTY — the good insert was rolled back.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "the atomic sub-batch roll-back must leave the table empty; found {} rows",
        count
    );
}

// ============================================================================
// Test 4: tx_in_tx_rejected
//
// A transactional sub-batch inside an already-open interactive tx →
// nested_tx_not_supported.
//
// We use `execute_in_open_tx` to drive a batch that contains a
// `BatchOp::Batch(transactional=true)` into an already-open TxContext.
// The runner has `tx: Some(...)` so the guard fires.
// ============================================================================

#[tokio::test]
async fn tx_in_tx_rejected() {
    use crate::query::batch::execute_in_open_tx;
    use crate::query::batch::open_interactive_tx;

    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Open an interactive tx (simulates the outer transactional context).
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // Inner batch: transactional.
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(shamir_query_types::write::InsertOp {
                insert_into: crate::query::TableRef::new("users"),
                values: vec![mpack!({ "name": "tx_in_tx" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(40),
        name: None,
        transactional: true, // inner tx — should be rejected
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    // Outer batch is NOT transactional itself — we drive it via the
    // already-open tx using execute_in_open_tx, so the runner gets
    // tx: Some(...) and hits the guard when it sees sub.batch.transactional.
    let outer_req = BatchRequest {
        id: QueryValue::Int(4),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let result = execute_in_open_tx(
        &outer_req,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;

    drop(tx);
    drop(guard);

    // The sub-batch guard fires: the runner has tx: Some(...) and
    // sub.batch.transactional == true → nested_tx_not_supported.
    let err = result.expect_err("transactional sub inside open tx must error");
    assert_eq!(
        err.code(),
        Some("nested_tx_not_supported"),
        "expected nested_tx_not_supported, got {:?}",
        err
    );
}

// ============================================================================
// Test 5: unbound_param_in_filter_is_silent_miss
//
// A $param used inside the inner filter with no binding → the param resolves
// to None, the filter matches nothing (0 records). Not an error — silent miss.
// ============================================================================

#[tokio::test]
async fn unbound_param_in_filter_is_silent_miss() {
    let resolver = setup().await;

    // Seed a row to ensure the table is not empty (makes the test meaningful).
    let mut seed = shamir_query_builder::batch::Batch::new();
    seed.id(0);
    seed.op_silent(
        "s",
        shamir_query_builder::write::insert("users")
            .row(shamir_query_builder::write::doc().set("score", 7i64)),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Inner batch: read where score == $param missing_param (not in bind).
    // $param resolves to None inside resolve_filter_value → comparison fails
    // → 0 records returned. No error.
    let inner_where = Filter::Eq {
        field: vec!["score".to_string()],
        value: FilterValue::Param {
            name: "missing_param".to_string(),
        },
    };
    let inner_read = shamir_query_types::read::ReadQuery {
        from: crate::query::TableRef::new("users"),
        r#where: Some(inner_where),
        select: shamir_query_types::read::Select::all(),
        order_by: None,
        pagination: shamir_query_types::read::Pagination::default(),
        group_by: None,
        count_total: false,
        temporal: shamir_query_types::read::Temporal::default(),
        with_version: false,
    };

    let mut inner_queries = new_map();
    inner_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(inner_read),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(50),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Sub-batch with EMPTY bind — so missing_param is not supplied.
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(5),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .expect("unbound $param inside filter silently misses — not an error");

    // The inner read returns 0 records (the $param was unresolvable → no match).
    let sub_val = resp.results["sub"].value.as_ref().unwrap();
    let inner_records = sub_val["r"]["records"].as_array().unwrap();
    assert_eq!(
        inner_records.len(),
        0,
        "unbound $param in filter must return 0 records (silent miss), got {:?}",
        inner_records
    );
}

// ============================================================================
// Test 5b: unbound_param_in_bind_errors
//
// $param in bind map (outer scope has no such param) → unbound_param error.
// ============================================================================

#[tokio::test]
async fn unbound_param_in_bind_errors() {
    let resolver = setup().await;

    // Inner batch with a trivial read.
    let mut inner_queries = new_map();
    inner_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(shamir_query_types::read::ReadQuery {
                from: crate::query::TableRef::new("users"),
                r#where: None,
                select: shamir_query_types::read::Select::all(),
                order_by: None,
                pagination: shamir_query_types::read::Pagination::default(),
                group_by: None,
                count_total: false,
                temporal: shamir_query_types::read::Temporal::default(),
                with_version: false,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(51),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Bind references a $param from the OUTER scope that doesn't exist.
    let mut bind = new_map();
    bind.insert(
        "x".to_string(),
        FilterValue::Param {
            name: "nonexistent_outer_param".to_string(),
        },
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(5),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let err = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("$param in bind with no outer scope must error");

    assert_eq!(
        err.code(),
        Some("unbound_param"),
        "expected unbound_param code, got {:?}",
        err
    );
}

// ============================================================================
// Test 6: param_in_insert_values
//
// P3b — canonical case: sub-batch inserts a row where a column value comes
// from a $param bound to the result of an outer query.
//
// Outer:
//   user → read user by name (returns id = 42)
//   sub  → BatchOp::Batch with bind: { uid: $query @user[0].id }
//            inner: insert into orders { user_id: {"$param":"uid"}, note: "order1" }
//   read_back → read orders where user_id == 42
//
// After execution, `read_back` must return exactly one record with user_id=42.
// ============================================================================

#[tokio::test]
async fn param_in_insert_values() {
    let resolver = setup().await;

    // Seed a user with id field set explicitly.
    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent(
        "seed_user",
        write::insert("users").row(doc().set("name", "p3b_alice").set("user_id", 42i64)),
    );
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Inner batch: insert one order using $param uid as column value.
    let inner_insert_value = mpack!({
        "user_id": { "$param": "uid" },
        "note": "order1"
    });
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![inner_insert_value],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(60),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Outer batch:
    //   user → read user with name p3b_alice (has user_id=42)
    //   sub  → inner batch with bind: { uid: $query @user[0].user_id }
    //   read_back → read orders where user_id == 42
    let outer_read_q =
        shamir_query_builder::query::Query::from("users").where_eq("name", "p3b_alice");
    let user_entry = QueryEntry {
        op: BatchOp::Read(crate::query::read::ReadQuery::from(outer_read_q)),
        return_result: true,
        after: Vec::new(),
    };

    let mut bind = new_map();
    bind.insert(
        "uid".to_string(),
        FilterValue::QueryRef {
            alias: "@user".to_string(),
            path: Some("[0].user_id".to_string()),
        },
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: vec!["user".to_string()],
    };

    let read_back_q = shamir_query_builder::query::Query::from("orders").where_eq("user_id", 42i64);
    let read_back_entry = QueryEntry {
        op: BatchOp::Read(crate::query::read::ReadQuery::from(read_back_q)),
        return_result: true,
        after: vec!["sub".to_string()],
    };

    let mut outer_queries = new_map();
    outer_queries.insert("user".to_string(), user_entry);
    outer_queries.insert("sub".to_string(), sub_entry);
    outer_queries.insert("read_back".to_string(), read_back_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(6),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let records = &resp.results["read_back"].records;
    assert_eq!(
        records.len(),
        1,
        "read_back must find exactly 1 order with user_id=42; got {:?}",
        records
    );
    assert_eq!(
        records[0].get_value_i64("user_id"),
        Some(42),
        "inserted order must have user_id == 42 (from $param uid)"
    );
    assert_eq!(
        records[0].get_value_str("note"),
        Some("order1"),
        "note field must be preserved verbatim"
    );
}

// ============================================================================
// Test 7: param_in_insert_nested
//
// P3b — $param resolves at depth inside an object nested in an insert value.
// ============================================================================

#[tokio::test]
async fn param_in_insert_nested() {
    let resolver = setup().await;

    // Inner batch: insert a row with a nested object containing $param.
    let inner_value = mpack!({
        "meta": {
            "created_by": { "$param": "actor_id" }
        },
        "label": "nested_test"
    });
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![inner_value],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(70),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Bind actor_id = 99.
    let mut bind = new_map();
    bind.insert(
        "actor_id".to_string(),
        // Literal 99 expressed as a FilterValue so the bind resolution
        // path exercises the full pipeline from FilterValue → InnerValue.
        FilterValue::Int(99),
    );
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind,
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(7),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // The inserted record's nested field must carry the resolved value.
    let sub_val = resp.results["sub"].value.as_ref().unwrap();
    let inserted = &sub_val["ins"]["records"];
    let rows = inserted.as_array().expect("ins.records must be array");
    assert_eq!(rows.len(), 1, "must have inserted 1 row");
    // The stored record is a flat record; nested objects may be serialised as
    // nested msgpack maps. We verify via a direct read.
    drop(resp);

    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders").where_eq("label", "nested_test"),
            )),
            return_result: true,
            after: Vec::new(),
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(71),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: read_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let read_resp = execute_batch(&read_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let rows = &read_resp.results["r"].records;
    assert_eq!(rows.len(), 1, "must find the inserted row by label");
    let row0_qv = rows[0].as_value();
    let created_by = row0_qv
        .get("meta")
        .and_then(|m| m.get("created_by"))
        .and_then(|v| v.as_i64());
    assert_eq!(
        created_by,
        Some(99),
        "nested $param must have been substituted to 99"
    );
}

// ============================================================================
// Test 8: param_in_insert_missing_param_errors
//
// P3b — an insert value references a $param that was NOT bound →
// unbound_param error (not a silent miss like in filters).
// ============================================================================

#[tokio::test]
async fn param_in_insert_missing_param_errors() {
    let resolver = setup().await;

    // Inner insert references $param that is NOT in bind.
    let inner_value = mpack!({
        "user_id": { "$param": "ghost_param" }
    });
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![inner_value],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(80),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: inner_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // Bind is EMPTY — ghost_param not supplied.
    let sub_entry = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner_req,
            bind: new_map(),
        }),
        return_result: true,
        after: Vec::new(),
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(8),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let err = execute_batch(&outer_req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("missing $param in insert value must error");

    assert_eq!(
        err.code(),
        Some("unbound_param"),
        "expected unbound_param, got {:?}",
        err
    );
}
