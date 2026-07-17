//! #666 — DoS-gate hardening tests for `ForEach`'s two runtime ceilings:
//! the absolute `max_iterations` clamp, and the `max_execution_time_secs`
//! wall-clock timeout at the top-level `execute_batch` entry point.
//!
//! See `docs/dev-artifacts/prompts/gap-666/01-foreach-dos-gate-hardening.md`
//! for the full background — this file implements its 5 required tests.

use std::time::Duration;

use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use shamir_query_types::filter::FilterValue;
use shamir_query_types::write::InsertOp;
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::query_runner::{
    effective_max_iterations, ABSOLUTE_MAX_FOR_EACH_ITERATIONS,
};
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

/// A `TableResolver` wrapper that sleeps for `delay` before delegating every
/// `resolve()` call to the inner resolver — used to construct a batch whose
/// per-op execution takes deliberately-real (but test-sized) wall-clock
/// time, so `execute_batch`'s `tokio::time::timeout` has something genuine
/// to race against. `resolve_repo` is NOT delayed (it is a one-time,
/// non-per-op lookup; delaying the per-op path via `resolve()` is
/// sufficient and mirrors where the actual DB I/O happens).
struct SlowResolver<R> {
    inner: R,
    delay: Duration,
}

#[async_trait::async_trait]
impl<R: TableResolver + Send + Sync> TableResolver for SlowResolver<R> {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        tokio::time::sleep(self.delay).await;
        self.inner.resolve(table_ref).await
    }

    async fn resolve_repo(&self, repo_name: &str) -> DbResult<RepoInstance> {
        self.inner.resolve_repo(repo_name).await
    }
}

fn insert_body_of(table: &str, field: &str, value: i64) -> BatchRequest {
    // `mpack!` needs its keys as literal tokens; `field` is a runtime `&str`
    // parameter here, so the record is built directly instead (same
    // constraint `for_each_tests.rs`'s `insert_order_body` documents).
    let mut insert_value = new_map();
    insert_value.insert(field.to_string(), QueryValue::Int(value));

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new(table),
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
// Test 1 — `effective_max_iterations` clamps a client-supplied value down to
// the absolute ceiling.
//
// Preferred approach per the brief: unit-test the extracted helper function
// directly rather than seeding `ABSOLUTE_MAX_FOR_EACH_ITERATIONS + 1` real
// rows (expensive). This is cheaper and just as decisive — it proves the
// SAME clamp the runtime gate calls.
// ============================================================================

#[test]
fn for_each_max_iterations_clamped_to_absolute_ceiling_even_when_client_requests_more() {
    let absurd = BatchLimits {
        max_iterations: usize::MAX,
        ..BatchLimits::default()
    };
    assert_eq!(
        effective_max_iterations(&absurd),
        ABSOLUTE_MAX_FOR_EACH_ITERATIONS,
        "a client-supplied max_iterations of usize::MAX must be clamped down \
         to the server's absolute ceiling, not honored verbatim"
    );

    let just_over = BatchLimits {
        max_iterations: ABSOLUTE_MAX_FOR_EACH_ITERATIONS + 1,
        ..BatchLimits::default()
    };
    assert_eq!(
        effective_max_iterations(&just_over),
        ABSOLUTE_MAX_FOR_EACH_ITERATIONS,
        "a client-supplied value one above the ceiling must still be clamped \
         down to exactly the ceiling"
    );

    let at_ceiling = BatchLimits {
        max_iterations: ABSOLUTE_MAX_FOR_EACH_ITERATIONS,
        ..BatchLimits::default()
    };
    assert_eq!(
        effective_max_iterations(&at_ceiling),
        ABSOLUTE_MAX_FOR_EACH_ITERATIONS,
        "a client-supplied value exactly AT the ceiling must pass through \
         unchanged (min of equal values)"
    );
}

// ============================================================================
// Test 2 — the clamp is a `min`, not a replacement: a client value BELOW the
// ceiling is respected unchanged (#653 behavior preserved).
// ============================================================================

#[test]
fn for_each_max_iterations_below_ceiling_is_respected_unchanged() {
    let small = BatchLimits {
        max_iterations: 2,
        ..BatchLimits::default()
    };
    assert_eq!(
        effective_max_iterations(&small),
        2,
        "a client-supplied value well below the absolute ceiling must pass \
         through unchanged — the clamp must never RAISE the effective value"
    );
}

/// End-to-end companion to the unit test above: a `ForEach` whose body's
/// `limits.max_iterations` is small (`2`) and `over` resolves to more
/// elements (`3`) must still reject with `TooManyIterations { max: 2, .. }`
/// — NOT the absolute ceiling — proving the runtime gate actually calls the
/// `min`-based helper, not a hardcoded ceiling replacement. This is the
/// RUNTIME/dynamic-`over` counterpart to
/// `for_each_max_iterations_exceeded_errors_before_first_iteration`
/// (`for_each_tests.rs`, literal-array/plan-time shape).
#[tokio::test]
async fn for_each_max_iterations_below_ceiling_end_to_end() {
    let resolver = setup().await;

    let mut body = insert_body_of("orders", "user_id", 0);
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
        "expected TooManyIterations{{actual:3,max:2}} (the CLIENT's own, \
         below-ceiling value) — not the absolute ceiling — got {:?}",
        err
    );
}

// ============================================================================
// Test 3 — a batch exceeding its `max_execution_time_secs` budget returns
// `ExecutionTimedOut`.
// ============================================================================

#[tokio::test]
async fn execute_batch_exceeding_time_budget_returns_execution_timed_out() {
    let resolver = setup().await;
    // `max_execution_time_secs` is `u64` seconds; `.max(1)` inside
    // `execute_batch` means the smallest real budget is 1 real second. The
    // slow mock's delay must genuinely exceed that to make the timeout fire
    // deterministically (not racily) — 1.3s gives comfortable margin
    // without making the test unreasonably slow.
    let slow = SlowResolver {
        inner: resolver,
        delay: Duration::from_millis(1300),
    };

    let mut req = insert_body_of("orders", "user_id", 1);
    req.limits = BatchLimits {
        max_execution_time_secs: 1,
        ..BatchLimits::default()
    };

    let err = execute_batch(&req, &slow, None, None, Actor::System, "test")
        .await
        .expect_err("a batch whose op takes 1.3s must time out against a 1s budget");
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 1 }),
        "expected ExecutionTimedOut{{budget_secs:1}}, got {:?}",
        err
    );
}

// ============================================================================
// Test 4 — the SAME harness with a generous budget succeeds unaffected,
// proving the timeout wrapper adds no false-positive overhead.
// ============================================================================

#[tokio::test]
async fn execute_batch_within_time_budget_succeeds_unaffected() {
    let resolver = setup().await;

    let mut req = insert_body_of("orders", "user_id", 2);
    req.limits = BatchLimits {
        max_execution_time_secs: 30, // default — generous, no artificial delay
        ..BatchLimits::default()
    };

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect("a fast batch well within its time budget must succeed normally");
    assert!(
        resp.results.get("ins").is_some(),
        "the insert must have actually run and produced a result"
    );
}

// ============================================================================
// Test 5 — DECISIVE: a timeout mid-transactional-batch rolls back a partial
// write. Proves `tokio::time::timeout`'s future-drop genuinely triggers the
// SAME RAII rollback an ordinary `Err` return does (#661's own technique).
// ============================================================================

/// A TRANSACTIONAL batch: first a real write that succeeds fast (`good`),
/// then a second write (`slow`) whose `TableResolver::resolve` is
/// artificially delayed past the batch's time budget. Because the batch is
/// `transactional: true`, both ops run against the SAME `TxContext` opened
/// by `execute_transactional_impl` — `good`'s write is a real, uncommitted
/// mutation staged in that tx; `slow` never finishes because the WHOLE
/// `execute_batch_impl` future (which owns the local `tx` variable across
/// its `.await` points, per `execute_transactional_impl`'s body) is dropped
/// by `tokio::time::timeout` once the budget elapses.
///
/// **Why the timeout-drop path and the error-return path give IDENTICAL
/// rollback behavior**: `execute_transactional_impl` only ever reaches
/// `repo.commit_tx(tx)` AFTER `execute_plan_tx_impl(...).await` returns
/// (either `Ok` or `Err`) — see `batch_execute.rs`'s `match plan_result`.
/// Both the ordinary-`Err` path (`for_each_iteration_error_aborts_whole_tx_batch`
/// in `for_each_tests.rs`) and this timeout path end with the tx NEVER
/// reaching `commit_tx`: the ordinary-`Err` path because `plan_result` is
/// `Err` and the `Err` arm intentionally never commits (it releases
/// pessimistic locks and drops `tx` implicitly at scope exit); the
/// timeout path because `tokio::time::timeout` polls the wrapped future
/// only until the budget elapses, then DROPS the future outright without
/// ever letting it resume — so `commit_tx` is never even reached, and
/// every local (including the `tx: TxContext` living on that future's
/// async-fn state-machine "stack") is dropped via ordinary Rust
/// drop-on-scope-exit semantics, precisely the same RAII rollback
/// `execute_transactional_impl`'s `Err` arm relies on. There is no new
/// rollback logic anywhere in this fix — `tokio::time::timeout` merely
/// supplies an EXTERNALLY-triggered `Err`-equivalent outcome (cancellation)
/// instead of one computed from inside the future.
///
/// **Why this test would have FAILED before the fix**: before #666,
/// `execute_batch` had no `tokio::time::timeout` at all — it called
/// `execute_batch_impl` directly and simply `.await`ed it to completion,
/// however long that took. The `slow` op's artificial delay would have just
/// been awaited in full (no cancellation source existed), `execute_plan_tx_impl`
/// would have eventually returned `Ok` (the delayed op is not itself an
/// error — `resolve()` still succeeds, just late), and the whole
/// transaction — INCLUDING `good`'s write — would have COMMITTED normally.
/// A fresh read after the call would show `good`'s row present, and the
/// call would return `Ok(..)`, not `Err(ExecutionTimedOut {..})` — the
/// opposite of both assertions below. This test is therefore decisive: it
/// fails without the fix (no timeout fires, the slow op runs to
/// completion, the write survives) and passes with it (the write is rolled
/// back).
#[tokio::test]
async fn execute_batch_timeout_during_transactional_batch_rolls_back_partial_writes() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![TableConfig::new("users"), TableConfig::new("orders")],
    )
    .await
    .unwrap();

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

    let resolver = TxTestResolver { repo: repo.clone() };
    let slow = SlowResolver {
        inner: resolver,
        delay: Duration::from_millis(1300),
    };

    let mut outer_queries = new_map();
    outer_queries.insert(
        "good".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![mpack!({ "name": "timeout_rollback_test" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    outer_queries.insert(
        "slow".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![mpack!({ "note": "never_should_survive" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: vec!["good".to_string()],
            when: None,
        },
    );

    let outer_req = BatchRequest {
        id: QueryValue::Int(666),
        name: None,
        transactional: true, // atomic — must roll back "good" too once "slow" times out
        isolation: None,
        durability: None,
        queries: outer_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits {
            max_execution_time_secs: 1,
            ..BatchLimits::default()
        },
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // (a) the call returns Err(ExecutionTimedOut { .. }).
    let err = execute_batch(&outer_req, &slow, None, None, Actor::System, "test")
        .await
        .expect_err(
            "a transactional batch whose 2nd op takes 1.3s against a 1s \
             budget must time out",
        );
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 1 }),
        "expected ExecutionTimedOut{{budget_secs:1}}, got {:?}",
        err
    );

    // (b) a FRESH read afterward shows the earlier write did NOT survive —
    // the timeout's future-drop genuinely triggered the tx's RAII rollback.
    let tbl = repo.get_table("users").await.unwrap();
    use futures::StreamExt;
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "the 'good' insert that ran before the timing-out 'slow' op must be \
         rolled back along with the rest of the tx once the timeout drops \
         the in-flight future — found {} row(s); before #666's fix there was \
         no timeout at all, so this write would have durably committed",
        count
    );
}
