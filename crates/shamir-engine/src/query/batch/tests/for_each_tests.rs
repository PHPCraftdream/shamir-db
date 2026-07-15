//! Epic04/B (#653) — `ForEachOp` K-times executor tests.
//!
//! Covers: 0/1/N iterations, `bind_row` availability inside the body as a
//! `$param`, `max_iterations` exceeded before iteration 0, and tx-abort
//! semantics when an iteration fails inside a transactional batch.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use shamir_query_types::filter::{FilterValue, FnCall};
use shamir_query_types::write::InsertOp;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch, BatchError, TableResolver};
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

/// Build a `for_each` loop body that inserts one order row per iteration,
/// with `user_id` sourced from the bound row element (`$param bind_row`).
fn insert_order_body(bind_row: &str) -> BatchRequest {
    // `mpack!` needs its keys/values as literal tokens — `bind_row` is a
    // runtime `&str` parameter (the $param NAME, which varies per test), so
    // the nested `{"$param": bind_row}` object is built directly rather
    // than through the macro.
    let mut param_obj = new_map();
    param_obj.insert("$param".to_string(), QueryValue::Str(bind_row.to_string()));
    let mut insert_value = new_map();
    insert_value.insert("user_id".to_string(), QueryValue::Map(param_obj));
    insert_value.insert("note".to_string(), QueryValue::Str("fe".to_string()));

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![QueryValue::Map(insert_value)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(100),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

fn wrap_for_each(fe: ForEachOp) -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

// ============================================================================
// Test 1: for_each_zero_iterations
// ============================================================================

#[tokio::test]
async fn for_each_zero_iterations() {
    let resolver = setup().await;

    let fe = ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let value = resp.results["loop"].value.as_ref().unwrap();
    let list = value.as_array().expect("loop value must be a List");
    assert_eq!(list.len(), 0, "zero elements → zero iterations");
}

// ============================================================================
// Test 2: for_each_one_iteration_binds_row
// ============================================================================

#[tokio::test]
async fn for_each_one_iteration_binds_row() {
    let resolver = setup().await;

    let fe = ForEachOp {
        over: FilterValue::Array(vec![FilterValue::Int(7)]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let value = resp.results["loop"].value.as_ref().unwrap();
    let list = value.as_array().expect("loop value must be a List");
    assert_eq!(list.len(), 1, "one element → one iteration");

    // Verify the order was actually inserted with user_id == 7 (bind_row
    // resolved and passed as $param into the body).
    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders").where_eq("user_id", 7i64),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(2),
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
    assert_eq!(
        read_resp.results["r"].records.len(),
        1,
        "bind_row's value must have been passed into the body as $param uid"
    );
}

// ============================================================================
// Test 3: for_each_n_iterations_accumulate_list
// ============================================================================

#[tokio::test]
async fn for_each_n_iterations_accumulate_list() {
    let resolver = setup().await;

    let fe = ForEachOp {
        over: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let value = resp.results["loop"].value.as_ref().unwrap();
    let list = value.as_array().expect("loop value must be a List");
    assert_eq!(list.len(), 3, "three elements → three iterations");

    // All three orders must exist.
    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent("noop", write::insert("users").row(doc().set("noop", true)));
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(3),
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
    assert_eq!(
        read_resp.results["r"].records.len(),
        3,
        "each of the 3 iterations must have inserted exactly 1 order row"
    );
}

// ============================================================================
// Test 4: for_each_max_iterations_exceeded_errors_before_first_iteration
// ============================================================================

#[tokio::test]
async fn for_each_max_iterations_exceeded_errors_before_first_iteration() {
    let resolver = setup().await;

    let mut body = insert_order_body("uid");
    body.limits = BatchLimits {
        max_iterations: 2,
        ..BatchLimits::default()
    };

    let fe = ForEachOp {
        over: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ]),
        bind_row: "uid".to_string(),
        batch: body,
    };
    let req = wrap_for_each(fe);

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("3 elements > max_iterations(2) must error before iteration 0");
    assert!(
        matches!(
            err,
            BatchError::TooManyIterations {
                actual: 3,
                max: 2,
                ..
            }
        ),
        "expected TooManyIterations{{actual:3,max:2}}, got {:?}",
        err
    );

    // No orders must have been inserted — the gate runs BEFORE iteration 0,
    // never a partial run.
    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(4),
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
    assert_eq!(
        read_resp.results["r"].records.len(),
        0,
        "no iteration must have run when max_iterations is exceeded up-front"
    );
}

// ============================================================================
// Test 5: for_each_iteration_error_aborts_whole_tx_batch
// ============================================================================

#[tokio::test]
async fn for_each_iteration_error_aborts_whole_tx_batch() {
    use futures::StreamExt;

    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Loop body: insert a row into a table that does NOT exist, so every
    // iteration fails. Wrapped in a transactional outer batch.
    let mut inner_queries = new_map();
    inner_queries.insert(
        "bad".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("nonexistent_table"),
                values: vec![shamir_types::mpack!({ "x": 1_i64 })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(200),
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

    let fe_entry = QueryEntry {
        op: BatchOp::ForEach(ForEachOp {
            over: FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]),
            bind_row: "uid".to_string(),
            batch: inner_req,
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };

    // Outer: first insert a real row, then the ForEach (which will fail).
    // Wrapped in a transactional batch — the ForEach failure must abort the
    // WHOLE batch, rolling back the first insert too.
    let mut outer_queries = new_map();
    outer_queries.insert(
        "good".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![shamir_types::mpack!({ "name": "fe_tx_test" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    outer_queries.insert("loop".to_string(), fe_entry);
    // Force "loop" to run after "good" so the good insert lands first.
    if let Some(entry) = outer_queries.get_mut("loop") {
        entry.after = vec!["good".to_string()];
    }

    let outer_req = BatchRequest {
        id: QueryValue::Int(5),
        name: None,
        transactional: true, // atomic — must roll back everything on failure
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
        .expect("execute_batch itself succeeds; the abort is reported via TransactionInfo");

    let tx_info = resp
        .transaction
        .as_ref()
        .expect("transactional batch must carry TransactionInfo");
    assert_eq!(
        tx_info.status, "aborted",
        "a for_each iteration failure must abort the whole tx batch, not commit a partial prefix"
    );

    // The table must be EMPTY — the "good" insert was rolled back too.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "tx abort must roll back the 'good' insert that ran before the failing for_each; found {} rows",
        count
    );
}

// ============================================================================
// Epic04/D (#655) — `over` resolution, all three sources + "resolved once"
// ============================================================================
//
// Test 1 above (`for_each_zero_iterations`) and tests 2/3/4 already cover the
// literal-array `over` source end to end. The tests below add the two
// remaining sources — `$query`-column-ref (`@alias[].field`) and `$fn` call
// — plus a dedicated test proving `over` is resolved EXACTLY ONCE, before
// the loop starts, never re-resolved per iteration.

/// `over` as a `$query`-column-ref: `@users[].score`. Seeds 3 users with
/// distinct scores, then for_each's over the column, inserting one order
/// tagged with each score.
#[tokio::test]
async fn for_each_over_query_column_ref() {
    let resolver = setup().await;

    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent("s1", write::insert("users").row(doc().set("score", 1i64)));
    seed.op_silent("s2", write::insert("users").row(doc().set("score", 2i64)));
    seed.op_silent("s3", write::insert("users").row(doc().set("score", 3i64)));
    execute_batch(&seed.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let fe = ForEachOp {
        over: FilterValue::QueryRef {
            alias: "@users".to_string(),
            path: Some("[].score".to_string()),
        },
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };

    let mut queries = new_map();
    queries.insert(
        "users".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("users"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: vec!["users".to_string()],
            when: None,
        },
    );
    let req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let iterations = resp.results["loop"]
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("for_each result must be a List, one entry per iteration");
    assert_eq!(
        iterations.len(),
        3,
        "column-ref over @users[].score must run once per seeded user; got {:?}",
        iterations
    );

    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(2),
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
    let mut ids: Vec<i64> = read_resp.results["r"]
        .records
        .iter()
        .map(|r| r.get_value_i64("user_id").unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 2, 3],
        "each iteration must bind the correct user's score via the column-ref over; got {:?}",
        ids
    );
}

/// `over` as a `$fn` call resolving to a `List`: `slice([100,200,300,400], 0, 3)`.
#[tokio::test]
async fn for_each_over_fn_call_array() {
    let resolver = setup().await;

    // `$fn: slice([100, 200, 300, 400], 0, 3)` -> [100, 200, 300].
    let over = FilterValue::FnCall {
        call: FnCall::complex(
            "arrays/slice",
            vec![
                FilterValue::Array(vec![
                    FilterValue::Int(100),
                    FilterValue::Int(200),
                    FilterValue::Int(300),
                    FilterValue::Int(400),
                ]),
                FilterValue::Int(0),
                FilterValue::Int(3),
            ],
        ),
    };

    let fe = ForEachOp {
        over,
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let iterations = resp.results["loop"]
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("for_each result must be a List, one entry per iteration");
    assert_eq!(
        iterations.len(),
        3,
        "$fn-call over (slice -> List) must run exactly 3 iterations; got {:?}",
        iterations
    );

    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(2),
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
    let mut ids: Vec<i64> = read_resp.results["r"]
        .records
        .iter()
        .map(|r| r.get_value_i64("user_id").unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![100, 200, 300],
        "each iteration must bind the correct $fn-produced element; got {:?}",
        ids
    );
}

/// `over` (a `$fn` call) resolves EXACTLY ONCE, before the loop — never
/// re-resolved per iteration.
///
/// `$fn: "now"` reads the wall clock (impure, non-deterministic — see
/// `shamir_funclib::datetime::register`'s `now` entry, `deterministic: false`).
/// We build `over` as `slice([now(), now(), now()], 0, 3)`: the three
/// `now()` sub-calls are resolved as `$fn` args in a single top-to-bottom
/// walk of `resolve_filter_query` while `over` itself is resolved, BEFORE
/// the loop starts (see `query_runner.rs`'s ForEach handling: `elements` is
/// computed once, then `for element in elements` merely iterates the
/// already-materialized `Vec<QueryValue>` — no call to
/// `resolve_filter_query`/`resolve_query_ref_column` occurs inside that
/// loop). Each loop iteration performs a real async DB insert. If `over`
/// were (bug) re-resolved once per iteration, the three captured
/// wall-clock reads would be spread across 3 real DB round-trips; because
/// they are all captured together in the SAME resolution pass, the spread
/// between the earliest and latest capture must be tiny.
#[tokio::test]
async fn for_each_over_resolves_exactly_once_before_loop() {
    let resolver = setup().await;

    let now_call = || FilterValue::FnCall {
        call: FnCall::simple("datetime/now"),
    };

    let over = FilterValue::FnCall {
        call: FnCall::complex(
            "arrays/slice",
            vec![
                FilterValue::Array(vec![now_call(), now_call(), now_call()]),
                FilterValue::Int(0),
                FilterValue::Int(3),
            ],
        ),
    };

    let before_loop_ms = wall_clock_ms();

    let fe = ForEachOp {
        over,
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let after_loop_ms = wall_clock_ms();

    let iterations = resp.results["loop"]
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("for_each result must be a List, one entry per iteration");
    assert_eq!(
        iterations.len(),
        3,
        "over must resolve to exactly 3 elements (fixed array length), \
         regardless of how many DB round-trips the loop body performs"
    );

    // Read back the 3 timestamps the loop body captured via $param uid.
    let mut read_queries = new_map();
    read_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(crate::query::read::ReadQuery::from(
                shamir_query_builder::query::Query::from("orders"),
            )),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let read_req = BatchRequest {
        id: QueryValue::Int(2),
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
    let mut ts: Vec<i64> = read_resp.results["r"]
        .records
        .iter()
        .map(|r| r.get_value_i64("user_id").unwrap())
        .collect();
    ts.sort_unstable();

    assert_eq!(
        ts.len(),
        3,
        "must have captured 3 timestamps, one per iteration"
    );

    // All 3 captured timestamps must fall within [before_loop_ms, after_loop_ms]
    // -- i.e. they were captured during THIS execution, not stale/default.
    for &t in &ts {
        assert!(
            t >= before_loop_ms && t <= after_loop_ms,
            "captured timestamp {} must fall within the test's wall-clock window [{}, {}]",
            t,
            before_loop_ms,
            after_loop_ms
        );
    }

    // Decisive assertion: the three now() calls are siblings inside the SAME
    // array literal, evaluated in the SAME resolve_filter_query pass (over's
    // slice(...) call resolves every arg via one top-to-bottom recursive
    // walk -- see resolve.rs's FnCall arm). If `over` were instead
    // re-resolved once per loop iteration (the bug this test guards
    // against), the 2nd and 3rd captured timestamps would be read AFTER the
    // 1st iteration's real async insert executed, spreading the 3 captures
    // across 3 real DB round-trips. Because they are all resolved together
    // up front, the spread between min and max must be tiny.
    let spread = ts.last().unwrap() - ts.first().unwrap();
    assert!(
        spread <= 5,
        "the 3 now() calls inside `over` must all be captured in the SAME \
         resolution pass (tight timestamp spread), not once per loop \
         iteration (which would spread them across 3 real DB round-trips); \
         got spread={} ms, timestamps={:?}",
        spread,
        ts
    );
}

/// Wall-clock epoch millis (no new crate dep -- mirrors
/// `shamir_funclib::datetime::now`'s definition without depending on it).
fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
