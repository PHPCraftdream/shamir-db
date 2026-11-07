use shamir_query_builder::batch::Batch;
use shamir_query_builder::write::{self, doc};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use shamir_query_types::filter::{FilterValue, FnCall};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use super::support::{insert_order_body, setup, setup_with_scalars, wrap_for_each};
use crate::query::batch::{execute_batch_unchecked as execute_batch, BatchError};

/// `over` as a `$query`-column-ref: `@users[].score`. Seeds 3 users with
/// distinct scores, then for_each's over the column, inserting one order
/// tagged with each score.
#[tokio::test]
async fn for_each_over_query_column_ref() {
    let resolver = setup().await;

    let mut seed = Batch::new();
    seed.id(0);
    seed.op_silent(
        "s1",
        write::insert("users").row(doc().set("score", 1i64).unwrap()),
    );
    seed.op_silent(
        "s2",
        write::insert("users").row(doc().set("score", 2i64).unwrap()),
    );
    seed.op_silent(
        "s3",
        write::insert("users").row(doc().set("score", 3i64).unwrap()),
    );
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
        visibility_wait: None,
        durability: None,
        index_visibility: None,
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
        "bind_row's value (7) must have been passed into the body as $param uid"
    );
}

// ============================================================================
// P2 perf fix (TASK_GROUPS.md group 22) -- plan/validate the `for_each` body
// ONCE, not once per iteration.
//
// `run_nested_body_in_outer_tx` (tx arm) and `execute_batch_impl`'s
// per-iteration non-tx arm used to call `BatchPlanner::plan` +
// `validate_tables` (async table resolution) + `validate_filter_depth`
// AGAIN on every single iteration, even though the body's SHAPE never
// changes across iterations -- only the bound `$param` value does. The fix
// hoists `plan_and_validate_batch` out of the loop in
// `QueryRunner::run`'s `ForEach` handling (`query_runner.rs`) so it runs
// exactly once, before iteration 0, and every iteration reuses the same
// `BatchPlan`.
//
// The tests below prove three things:
//   1. A multi-query body still produces byte-identical results across N
//      iterations (no behavior change from the hoist).
//   2. A body that references a nonexistent table still fails validation,
//      and does so BEFORE any iteration's real work runs (zero rows
//      written) -- not silently skipped by the hoist.
//   3. The hoist actually eliminates the redundant work: a resolver call
//      counter shows `validate_tables` resolving the body's table(s)
//      exactly ONCE for the whole loop, not once per iteration.
// ============================================================================
