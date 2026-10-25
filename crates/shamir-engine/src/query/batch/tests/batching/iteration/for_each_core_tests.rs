use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::write::InsertOp;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::support::{
    insert_order_body, insert_order_body_with_div_guard, setup, wrap_for_each, TxTestResolver,
};
use crate::query::batch::{execute_batch_unchecked as execute_batch, BatchError};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoInstance;
use crate::table::TableConfig;

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
    seed.op_silent(
        "noop",
        write::insert("users").row(doc().set("noop", true).unwrap()),
    );
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
