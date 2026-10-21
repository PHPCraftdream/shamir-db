//! P3 — nested sub-batch executor tests.
//!
//! Covers: bind resolution, param injection, tx-in-tx guard,
//! unbound_param error, and sub-batch atomicity.

use shamir_funclib::registry::{FnEntry, ScalarResult};
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, QueryEntry, ResultEncoding, SubBatchOp,
};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::{execute_batch_unchecked as execute_batch, TableResolver};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbResult;

// ============================================================================
// Shared infrastructure
// ============================================================================

pub(super) struct TestResolver {
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

pub(super) async fn setup() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

pub(super) struct TxTestResolver {
    pub(super) repo: RepoInstance,
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
// Fix 2 — resolver variant with a user-registered scalar for bind tests.
// ============================================================================

/// Build a ScalarResolver with a user-registered scalar `my_double`.
fn resolver_with_user_scalar() -> ScalarResolver {
    let layer = UserScalarLayer::new();
    layer.register(
        "my_double",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::Int(n * 2)),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        ),
    );
    ScalarResolver::new(std::sync::Arc::new(layer))
}

pub(super) struct TestResolverWithScalars {
    db: DbInstance,
    scalars: ScalarResolver,
}

#[async_trait::async_trait]
impl TableResolver for TestResolverWithScalars {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }

    fn scalar_resolver(&self) -> ScalarResolver {
        self.scalars.clone()
    }
}

pub(super) async fn setup_with_scalars() -> TestResolverWithScalars {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolverWithScalars {
        db,
        scalars: resolver_with_user_scalar(),
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
        when: None,
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
            when: None,
        },
    );

    let outer_req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        explain: false,
    };

    let mut inner_queries = new_map();
    inner_queries.insert(
        "match".to_string(),
        QueryEntry {
            op: BatchOp::Read(inner_read),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(20),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        when: None,
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
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("user".to_string(), outer_user_entry);
    outer_queries.insert("sub".to_string(), sub_entry);

    let outer_req = BatchRequest {
        id: QueryValue::Int(2),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
            when: None,
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
            when: None,
        },
    );

    let inner_req = BatchRequest {
        id: QueryValue::Int(30),
        name: None,
        transactional: true, // atomic unit
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(3),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
// #661 — thread the outer tx into Batch (sub-batch) body recursion.
//
// `sub_batch_atomic` above covers a sub-batch that is ITSELF
// `transactional: true` — a fully independent nested transaction, which
// never touches this bug (its recursion path is `execute_batch_impl` ->
// `execute_transactional_impl`, opening its OWN tx; #661 only affects the
// case where the sub-batch body is NOT itself transactional but runs
// nested inside an ALREADY-OPEN outer transaction). This test exercises
// exactly that gap: a plain (non-transactional) `Batch(SubBatchOp)` whose
// body succeeds (a real write), running inside a TRANSACTIONAL outer batch,
// followed by a top-level sibling op that fails. Before #661's fix, the
// sub-batch's write would have routed through `execute_batch_impl`'s
// non-transactional path (an implicit, independently-committing single-op
// transaction) — entirely disconnected from the outer TxContext — so it
// would durably survive the outer tx's abort. After the fix, the write
// flows through the SAME outer `TxContext`, so the outer tx's RAII
// rollback-on-error genuinely undoes it.
// ============================================================================

#[tokio::test]
async fn sub_batch_partial_writes_roll_back_on_later_top_level_failure() {
    use futures::StreamExt;

    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    // Inner (sub-batch) body: a single, non-transactional insert that
    // SUCCEEDS.
    let mut inner_queries = new_map();
    inner_queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(shamir_query_types::write::InsertOp {
                insert_into: crate::query::TableRef::new("users"),
                values: vec![mpack!({ "name": "sub_batch_partial_test" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(6611),
        name: None,
        transactional: false, // the body itself is NOT transactional
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        when: None,
    };

    // Top-level sibling op that fails, forced to run AFTER the sub-batch so
    // the sub-batch's write genuinely happens first.
    //
    // IMPORTANT: this must NOT be a static/plan-time failure (like a
    // nonexistent table) — `execute_in_open_tx`/`execute_batch`'s
    // `validate_tables` call validates EVERY table ref across the WHOLE
    // outer `request.queries` map BEFORE any op executes (including the
    // sub-batch's own body), so a nonexistent-table sibling would reject
    // the ENTIRE outer batch up front and the sub-batch's insert would
    // never run at all — that would not exercise the recursion this test
    // targets. Instead, use a genuine RUNTIME, data-dependent failure: an
    // Insert whose value contains `$fn: math/mod(1, 0)`, which resolves
    // successfully past `validate_tables` (the `users` table is real) but
    // fails during marker resolution at execution time with `div_by_zero`
    // (see `crates/shamir-funclib/src/math.rs`'s `"mod"` entry and
    // `crates/shamir-engine/src/query/batch/param_subst.rs`'s
    // `WriteValueError::MalformedMarker` conversion) — same technique as
    // `for_each_tests.rs`'s `insert_order_body_with_div_guard`.
    let mut fn_args = new_map();
    fn_args.insert("name".to_string(), QueryValue::Str("math/mod".to_string()));
    fn_args.insert(
        "args".to_string(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Int(0)]),
    );
    let mut fn_marker = new_map();
    fn_marker.insert("$fn".to_string(), QueryValue::Map(fn_args));
    let mut bad_value = new_map();
    bad_value.insert("guard".to_string(), QueryValue::Map(fn_marker));
    let bad_entry = QueryEntry {
        op: BatchOp::Insert(shamir_query_types::write::InsertOp {
            insert_into: crate::query::TableRef::new("users"),
            values: vec![QueryValue::Map(bad_value)],
            records_idmsgpack: Vec::new(),
            select: None,
        }),
        return_result: true,
        after: vec!["sub".to_string()],
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    outer_queries.insert("bad".to_string(), bad_entry);

    // Outer batch is NOT transactional itself here — we drive it via an
    // already-open tx using `execute_in_open_tx` (same pattern
    // `tx_in_tx_rejected` below uses), so the runner sees `tx: Some(...)`
    // for the sub-batch recursion, exactly like a genuinely transactional
    // outer batch would.
    let outer_req = BatchRequest {
        id: QueryValue::Int(661),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let (mut tx, guard) =
        crate::query::batch::open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();

    let result = crate::query::batch::execute_in_open_tx_unchecked(
        &outer_req,
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;

    let err =
        result.expect_err("the top-level 'bad' op's failure must propagate as an error; got Ok");
    assert_eq!(
        err.code(),
        Some("malformed_marker"),
        "the failure must genuinely originate from the 'bad' op's runtime \
         math/mod div_by_zero (not from an unrelated up-front validation \
         failure) — proving the sub-batch's insert ran to completion \
         first; got {:?}",
        err
    );

    // Roll back the interactive tx exactly like a real caller would on
    // error (dropping the guard/tx without committing).
    drop(tx);
    drop(guard);

    // Decisive assertion (the one #661 broke): the table must be EMPTY —
    // the sub-batch's already-succeeded write must be genuinely rolled
    // back along with the rest of the outer tx, not durably committed
    // independently via its own implicit per-op transaction.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "the sub-batch's write must be rolled back along with the rest of \
         the outer tx; found {} row(s) — this is the exact case #661 broke: \
         a non-transactional sub-batch body silently committing its writes \
         independently of an already-open outer transaction",
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
    use crate::query::batch::execute_in_open_tx_unchecked as execute_in_open_tx;
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
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(40),
        name: None,
        transactional: true, // inner tx — should be rejected
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        when: None,
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
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        explain: false,
    };

    let mut inner_queries = new_map();
    inner_queries.insert(
        "r".to_string(),
        QueryEntry {
            op: BatchOp::Read(inner_read),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(50),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(5),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
                explain: false,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let inner_req = BatchRequest {
        id: QueryValue::Int(51),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        when: None,
    };

    let mut outer_queries = new_map();
    outer_queries.insert("sub".to_string(), sub_entry);
    let outer_req = BatchRequest {
        id: QueryValue::Int(5),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
