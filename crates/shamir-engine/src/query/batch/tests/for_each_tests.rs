//! Epic04/B (#653) — `ForEachOp` K-times executor tests.
//!
//! Covers: 0/1/N iterations, `bind_row` availability inside the body as a
//! `$param`, `max_iterations` exceeded before iteration 0, and tx-abort
//! semantics when an iteration fails inside a transactional batch.

use shamir_funclib::registry::{FnEntry, ScalarResult};
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
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

// ============================================================================
// Fix 2 — resolver variant with a user-registered scalar for over tests.
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
    // A second scalar that returns a singleton List — used by the
    // `for_each` `over` test, where `over` must resolve to `QueryValue::List`.
    layer.register(
        "make_singleton_list",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::List(vec![QueryValue::Int(*n)])),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        ),
    );
    ScalarResolver::new(std::sync::Arc::new(layer))
}

struct TestResolverWithScalars {
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

async fn setup_with_scalars() -> TestResolverWithScalars {
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

/// Build a `for_each` loop body like [`insert_order_body`], but with an
/// EXTRA `guard` field computed via `$fn: math/mod(1, $param <bind_row>)`
/// (`crates/shamir-funclib/src/math.rs`'s `"mod"` entry, dispatch name
/// `"math/mod"`, registered `FnEntry::pure`). `math/mod` returns
/// `Err(ScalarError::new("div_by_zero"))` when its divisor argument is
/// zero — `resolve_filter_query`'s `FnCall` arm collapses any scalar error
/// to `None` (`crates/shamir-engine/src/query/filter/resolve.rs`), and
/// `resolve_write_value`'s marker resolution turns that `None` into a hard
/// `WriteValueError::MalformedMarker` (`crates/shamir-engine/src/query/batch/
/// param_subst.rs`), which surfaces as a genuine `Err` out of the Insert
/// dispatch — NOT a silent skip.
///
/// This gives a PURE function of the current iteration's own `bind_row`
/// value (no cross-iteration state observation needed, unlike a unique
/// index — see this module's `#661` test section below for why a
/// unique-index-based mid-tx failure does NOT work across iterations of
/// the SAME uncommitted transaction: unique-key validation only checks
/// against DURABLE committed state, both at insert-time
/// (`validate_unique_for_create`) and at commit-time re-validation
/// (`tx::pre_commit::pre_commit_prelock`'s per-guard check against
/// `info_store`) — two inserts claiming the same key within ONE
/// still-uncommitted tx never cross-check each other and both silently
/// pass). A bind_row value of `0` makes THIS iteration's own insert fail,
/// independent of what any other iteration did.
fn insert_order_body_with_div_guard(bind_row: &str) -> BatchRequest {
    let mut param_obj = new_map();
    param_obj.insert("$param".to_string(), QueryValue::Str(bind_row.to_string()));
    let mut fn_args = new_map();
    fn_args.insert("name".to_string(), QueryValue::Str("math/mod".to_string()));
    fn_args.insert(
        "args".to_string(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Map(param_obj.clone())]),
    );
    let mut fn_marker = new_map();
    fn_marker.insert("$fn".to_string(), QueryValue::Map(fn_args));

    let mut insert_value = new_map();
    insert_value.insert("user_id".to_string(), QueryValue::Map(param_obj));
    insert_value.insert("note".to_string(), QueryValue::Str("fe".to_string()));
    insert_value.insert("guard".to_string(), QueryValue::Map(fn_marker));

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
        id: QueryValue::Int(101),
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
// #661 — thread the outer tx into ForEach body recursion.
//
// `for_each_iteration_error_aborts_whole_tx_batch` above makes EVERY
// iteration fail at validation time (table `nonexistent_table` never
// exists), so it never actually exercised "a successful iteration's write
// gets rolled back" -- only "zero successful iterations happened, and the
// pre-loop 'good' insert rolled back". Before #661's fix, a nested ForEach
// body reused the outer TxContext ONLY conceptually: `execute_batch_impl`
// always independently decided transactional-vs-not from the body's own
// `transactional` flag, so with `body.transactional == false` (the only
// reachable case once `nested_tx_not_supported` rejects the other combo)
// each iteration's write committed through its own implicit per-op
// transaction, entirely disconnected from the outer TxContext -- a
// genuinely-successful iteration 0 would durably survive even though the
// outer batch reports "aborted". This section adds the DATA-DEPENDENT
// mid-loop-failure case that would have caught that gap.
// ============================================================================

/// A TRANSACTIONAL outer batch containing a `ForEach` over `[1, 0, 2]`
/// (`bind_row` = `uid`) where:
///   - iteration 0 (`uid=1`) SUCCEEDS -- a real row is written (`user_id=1`).
///   - iteration 1 (`uid=0`) FAILS on a genuine data-dependent condition:
///     the body's own `guard` field is `$fn: math/mod(1, $param uid)`,
///     which errors with `div_by_zero` when `uid == 0` (see
///     `insert_order_body_with_div_guard`'s doc comment for why this
///     technique is used instead of a unique-index violation — the
///     "obvious" unique-index approach does NOT actually detect a conflict
///     across iterations of the SAME still-open transaction, since
///     unique-key validation only ever checks against DURABLE committed
///     state, never against another iteration's own uncommitted stage in
///     the same tx).
///   - iteration 2 never runs.
///
/// Before #661's fix, this test would FAIL: iteration 0's insert routed
/// through `execute_batch_impl`'s non-transactional path (an implicit,
/// independently-committing single-op transaction) because the ForEach
/// body itself has `transactional: false` -- entirely disconnected from
/// the outer TxContext. The outer batch would still report
/// `tx_info.status == "aborted"` (the top-level `execute_transactional_impl`
/// correctly aborts ITS OWN tx, which touched nothing), but iteration 0's
/// row would durably survive in the table regardless, because it was never
/// part of that tx to begin with. This test's decisive assertion -- a
/// FRESH read of `orders` shows ZERO rows after the abort -- is exactly the
/// assertion the old code could not satisfy. After the fix, iteration 0's
/// write flows through the SAME outer `TxContext` (via
/// `run_nested_body_in_outer_tx`/`execute_plan_tx_impl`), so the outer
/// tx's RAII rollback-on-error (dropping the tx without commit) genuinely
/// undoes it.
#[tokio::test]
async fn for_each_partial_iterations_roll_back_on_later_failure() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![TableConfig::new("users"), TableConfig::new("orders")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    let fe = ForEachOp {
        over: FilterValue::Array(vec![
            FilterValue::Int(1), // iteration 0: succeeds, writes user_id=1
            FilterValue::Int(0), // iteration 1: math/mod(1, 0) -> div_by_zero -> Err
            FilterValue::Int(2), // must never run
        ]),
        bind_row: "uid".to_string(),
        batch: insert_order_body_with_div_guard("uid"),
    };

    let mut outer_queries = new_map();
    outer_queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let outer_req = BatchRequest {
        id: QueryValue::Int(661),
        name: None,
        transactional: true, // atomic — must roll back iteration 0's write
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
        "iteration 1's math/mod div_by_zero failure must abort the whole tx batch"
    );

    // Decisive assertion (the one #661 broke): a FRESH read of `orders`
    // must show ZERO rows — iteration 0's already-succeeded write must be
    // genuinely rolled back along with the rest of the tx, not durably
    // committed independently.
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
        id: QueryValue::Int(662),
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
        "iteration 0's write must be rolled back along with the rest of the \
         outer tx — found {} row(s); this is the exact case #661 broke: an \
         already-succeeded iteration silently committing independently of \
         the outer transaction",
        read_resp.results["r"].records.len()
    );
}

/// Positive/happy-path regression guard: a TRANSACTIONAL outer batch with a
/// multi-iteration `ForEach` where EVERY iteration succeeds must still
/// COMMIT normally and all rows must be visible afterward — the #661 fix
/// (threading the outer tx through nested recursion) must not break the
/// common, all-succeed case.
#[tokio::test]
async fn for_each_all_iterations_succeed_commits_and_all_rows_visible() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![TableConfig::new("users"), TableConfig::new("orders")],
    )
    .await
    .unwrap();
    let resolver = TxTestResolver { repo: repo.clone() };

    let fe = ForEachOp {
        over: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };

    let mut outer_queries = new_map();
    outer_queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let outer_req = BatchRequest {
        id: QueryValue::Int(663),
        name: None,
        transactional: true,
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
        .expect("all-iterations-succeed for_each must not error");

    let tx_info = resp
        .transaction
        .as_ref()
        .expect("transactional batch must carry TransactionInfo");
    assert_eq!(
        tx_info.status, "committed",
        "when every iteration succeeds, the outer tx must commit normally"
    );

    let list = resp.results["loop"]
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(list.len(), 3, "three elements → three iterations");

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
        id: QueryValue::Int(664),
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
        "all 3 committed iterations' rows must be visible after the tx commits"
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

// ============================================================================
// Epic04/D (#655) gap 2 -- non-transactional stop-at-first semantics
// (ADR Decision 4: autocommit batch is stop-at-first, NOT collect-errors).
// ============================================================================

/// A `ForEach` inside a NON-transactional (autocommit) outer batch: once
/// iteration `i` fails, iteration `i+1..K` never runs, but the writes from
/// iterations `0..i-1` are NOT rolled back (there is no transaction to roll
/// back). Mirrors `for_each_iteration_error_aborts_whole_tx_batch` but with
/// `transactional: false` on the outer batch and asserts the opposite
/// durability outcome for the already-applied prefix.
///
/// Failure at iteration 1 is made data-dependent (not table-existence
/// based, so it fires mid-loop rather than on iteration 0): a unique index
/// on `orders.user_id` plus `over: [1, 1, 2]` -- iteration 0 inserts
/// user_id=1 (ok), iteration 1 duplicates user_id=1 (DuplicateKey error),
/// iteration 2 (user_id=2) never runs.
#[tokio::test]
async fn for_each_iteration_error_stops_at_first_in_non_tx_batch() {
    let resolver = setup().await;
    resolver
        .db
        .create_unique_index("default", "orders", "orders_user_id_uq", &["user_id"])
        .await
        .expect("unique index creation must succeed");

    let fe = ForEachOp {
        over: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(1), // duplicates iteration 0 -> fails here
            FilterValue::Int(2), // must never run
        ]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe); // wrap_for_each builds transactional: false

    let err = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect_err("iteration 1's duplicate user_id must fail the batch");
    assert!(
        matches!(err, BatchError::QueryError { .. }),
        "expected a QueryError surfacing the duplicate-key failure at the \
         failing iteration, got {:?}",
        err
    );

    // The iteration-0 write MUST be visible: no transaction to roll it
    // back in the non-tx (autocommit) case.
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
    let ids: Vec<i64> = read_resp.results["r"]
        .records
        .iter()
        .map(|r| r.get_value_i64("user_id").unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![1],
        "iteration 0's write must survive (no tx to roll back) and \
         iteration 2 (user_id=2) must never have run (stop-at-first), got {:?}",
        ids
    );
}

// ============================================================================
// Epic04/D (#655) gap 3 -- pessimistic authorization: a ForEach whose body
// is a write must be classified/rejected as a write EVEN when `over`
// resolves to zero iterations at runtime (ADR Decision 5).
// ============================================================================
//
// The real production gate this mirrors is `shamir-server`'s read-only-
// replica check (`db_handler/handler.rs`: `if entry.op.is_write() { reject
// }`), which iterates the OUTER batch's top-level `QueryEntry`s and rejects
// any whose `is_write()` is true -- entirely independent of what actually
// runs at execution time. That gate lives outside this crate's scope, but
// its authorization DECISION is exactly `BatchOp::is_write()`
// (`shamir-query-types`), so this test simulates the identical gate loop
// directly against a `ForEach` entry to pin the "rejected even with zero
// runtime iterations" guarantee at the engine layer, one level above the
// pure op-level unit test in `shamir-query-types`'s `for_each_op_tests.rs`.

/// Simulates a write-rejecting authorization gate (the same shape as
/// `shamir-server`'s read-only-replica check): rejects the batch if ANY
/// top-level entry's `op.is_write()` is true.
fn simulate_write_rejecting_gate(
    queries: &shamir_types::types::common::TMap<String, QueryEntry>,
) -> Result<(), String> {
    for (alias, entry) in queries {
        if entry.op.is_write() {
            return Err(format!("query '{}' is a write; rejected", alias));
        }
    }
    Ok(())
}

#[tokio::test]
async fn for_each_write_body_rejected_by_pessimistic_gate_even_with_zero_iterations() {
    // `over` is a literal empty array: ZERO iterations will actually run if
    // this were allowed to execute. The gate must still reject it, because
    // is_write() classifies over the FULL static body, not the runtime
    // iteration count -- this is the plan-time-conservative model ADR
    // Decision 5 mandates, mirroring Epic03's `when` precedent.
    let fe = ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let result = simulate_write_rejecting_gate(&req.queries);
    assert!(
        result.is_err(),
        "a ForEach with a write body must be rejected by a write-gate even \
         when `over` resolves to zero iterations at runtime"
    );

    // Sanity: confirm this is genuinely a zero-iteration case by actually
    // running it (unguarded) and observing 0 iterations -- otherwise this
    // test would not be exercising the "zero iterations, still rejected"
    // edge case the ADR calls out.
    let resolver = setup().await;
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let list = resp.results["loop"]
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(
        list.len(),
        0,
        "sanity check: `over: []` really does produce zero iterations"
    );
}

#[tokio::test]
async fn for_each_read_only_body_passes_pessimistic_gate() {
    // Contrast case: a ForEach whose body is pure reads must NOT be
    // rejected by the same write-gate.
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
    let read_body = BatchRequest {
        id: QueryValue::Int(100),
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
    let fe = ForEachOp {
        over: FilterValue::Array(vec![]),
        bind_row: "uid".to_string(),
        batch: read_body,
    };
    let req = wrap_for_each(fe);

    let result = simulate_write_rejecting_gate(&req.queries);
    assert!(
        result.is_ok(),
        "a ForEach with a pure-read body must pass a write-gate, got {:?}",
        result
    );
}

// ============================================================================
// Epic04/D (#655) gap 4 -- `when` (Epic03) on the ForEach entry itself.
// ============================================================================
//
// A `ForEach` entry is a normal `QueryEntry`, so `when: Option<Filter>`
// applies to it exactly like any other op (Epic03/B, #645). When the
// ForEach's own `when` evaluates false, the ENTIRE loop is skipped --
// `skipped: true`, zero iterations -- via the `when`-skip codepath
// (`resolve_skip`, evaluated BEFORE `over` is ever resolved), which is a
// distinct codepath from "ran 0 iterations because `over` happened to be
// an empty array" (that path still executes the ForEach node itself and
// returns `skipped: false, value: List([])`).

/// `when: false` on a ForEach entry: the whole loop is skipped -- no
/// iterations run (not even iteration 0), the result is `skipped: true`
/// with no `value` -- distinguishable from "0 real iterations" which
/// still produces `skipped: false, value: List([])` (see
/// `for_each_zero_iterations` above).
#[tokio::test]
async fn for_each_when_false_skips_entire_loop_not_zero_iterations() {
    let resolver = setup().await;

    let fe = ForEachOp {
        // A non-empty `over` -- if `when` did NOT gate this correctly and
        // the loop ran, iterations would actually execute and insert rows.
        over: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let mut req = wrap_for_each(fe);
    // `IsNotNull` on a field that never exists on the empty synthetic
    // record used for `when` evaluation is always false (same pattern as
    // `when_false_skips_op` in `when_skip_tests.rs`).
    req.queries.get_mut("loop").unwrap().when =
        Some(shamir_query_types::filter::Filter::IsNotNull {
            field: vec!["anything".to_string()],
        });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let result = &resp.results["loop"];
    assert!(
        result.skipped,
        "ForEach's own `when` evaluating false must skip the ENTIRE loop \
         (skipped: true), not merely produce zero iterations"
    );
    assert!(
        result.value.is_none(),
        "a genuinely skipped ForEach must carry no `value` at all -- \
         distinct from a 0-element List, which a real (unskipped) 0-\
         iteration run would produce"
    );

    // Decisive: no iterations ran -- the "over" array had 3 elements, so if
    // `when`-skip were (bug) implemented as "resolve over, then run 0
    // iterations" rather than "skip before resolving over at all", this
    // assertion alone wouldn't catch it (over's length never gates
    // iteration count either way here) -- but no orders were inserted
    // proves the body never executed, which only a genuine skip achieves.
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
    assert_eq!(
        read_resp.results["r"].records.len(),
        0,
        "a skipped ForEach must never have run its body at all -- 0 orders \
         must exist, even though `over` had 3 elements"
    );
}

/// Contrast case: `when: true` on a ForEach entry executes the loop
/// normally -- `skipped: false`, real iterations run.
#[tokio::test]
async fn for_each_when_true_runs_loop_normally() {
    let resolver = setup().await;

    let fe = ForEachOp {
        over: FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Int(2)]),
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let mut req = wrap_for_each(fe);
    // `IsNull` on a field that can never be present on the empty synthetic
    // record is always true.
    req.queries.get_mut("loop").unwrap().when = Some(shamir_query_types::filter::Filter::IsNull {
        field: vec!["anything".to_string()],
    });

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let result = &resp.results["loop"];
    assert!(
        !result.skipped,
        "ForEach's own `when` evaluating true must execute the loop normally"
    );
    let list = result
        .value
        .as_ref()
        .unwrap()
        .as_array()
        .expect("loop value must be a List");
    assert_eq!(list.len(), 2, "both iterations must have run");
}

// ============================================================================
// Fix 2 (Finding 8) — user-registered scalar in `for_each`'s `over` expression.
// ============================================================================

/// `for_each`'s `over` expression uses a user-registered scalar
/// (`$fn: make_singleton_list(7)`). With the scalar resolver threaded into
/// the over-resolution `FilterContext` (via
/// `.with_scalars(self.resolver.scalar_resolver())` at ~line 504 in
/// `query_runner.rs`), `make_singleton_list(7)` resolves to
/// `List([Int(7)])`, the loop iterates once with `bind_row = 7`, and the
/// body inserts an order with `user_id = 7`.
///
/// **Pre-fix behavior** (builtins-only `ScalarResolver`):
/// `make_singleton_list` is an unknown function → `resolve_filter_query`
/// returns `None` → the `over` resolution arm hits `Some(_) | None =>` and
/// returns `Err(BatchError::QueryError { "for_each 'loop': 'over' did not
/// resolve to a list" })`. The test would fail at
/// `resp.unwrap()` because the batch would return `Err`.
#[tokio::test]
async fn for_each_over_user_scalar_resolves_correctly() {
    let resolver = setup_with_scalars().await;

    // `over` = $fn: make_singleton_list(7) → List([7]) → 1 iteration.
    let fe = ForEachOp {
        over: FilterValue::FnCall {
            call: FnCall::complex("make_singleton_list", vec![FilterValue::Int(7)]),
        },
        bind_row: "uid".to_string(),
        batch: insert_order_body("uid"),
    };
    let req = wrap_for_each(fe);

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let value = resp.results["loop"].value.as_ref().unwrap();
    let list = value.as_array().expect("loop value must be a List");
    assert_eq!(
        list.len(),
        1,
        "make_singleton_list(7) → List([7]) → exactly 1 iteration"
    );

    // Verify the order was actually inserted with user_id == 7 (bind_row
    // resolved from the scalar-produced list element).
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
        "bind_row's value (7) must have been passed into the body as $param uid"
    );
}
