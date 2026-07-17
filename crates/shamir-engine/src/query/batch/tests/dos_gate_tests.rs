//! #666 — DoS-gate hardening tests for `ForEach`'s two runtime ceilings:
//! the absolute `max_iterations` clamp, and the `max_execution_time_secs`
//! wall-clock budget enforced via COOPERATIVE deadline checkpoints
//! (`ExecutionDeadline`, threaded from the top-level `execute_batch` entry
//! point through the whole recursive execution — see
//! `execution_deadline.rs` for why the original preemptive
//! `tokio::time::timeout` wrapper was replaced).
//!
//! See `docs/dev-artifacts/prompts/gap-666/01-foreach-dos-gate-hardening.md`
//! and `docs/dev-artifacts/prompts/gap-666-followup/
//! 01-cancel-safe-execution-timeout-redesign.md` for the full background.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use shamir_query_builder::filter;
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
/// time, so the cooperative `ExecutionDeadline` checkpoints observe a
/// genuinely-elapsed budget at the next unit-of-work boundary.
/// `resolve_repo` is NOT delayed (it is a one-time, non-per-op lookup;
/// delaying the per-op path via `resolve()` is sufficient and mirrors where
/// the actual DB I/O happens).
///
/// `calls` counts every `resolve()` entry — lets a test assert how far
/// execution provably progressed before the deadline checkpoint fired
/// (e.g. "at least one full `ForEach` iteration ran").
struct SlowResolver<R> {
    inner: R,
    delay: Duration,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl<R: TableResolver + Send + Sync> TableResolver for SlowResolver<R> {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.inner.resolve(table_ref).await
    }

    async fn resolve_repo(&self, repo_name: &str) -> DbResult<RepoInstance> {
        self.inner.resolve_repo(repo_name).await
    }
}

/// A `TableResolver` wrapper that delays ONLY the second-and-later
/// `resolve()` calls for one specific table (`slow_table`), leaving the
/// FIRST call for that table — the one `validate_tables` makes up-front,
/// before `begin_tx` — fast.
///
/// This is the surgical fix for the vacuity the original rollback test had:
/// delaying EVERY `resolve()` meant the budget elapsed during up-front
/// validation, so `begin_tx` was never reached and "no partial rows
/// survive" passed trivially. With this resolver the delay lands on the
/// OP-dispatch resolve inside the already-open transaction, so earlier ops
/// provably run and stage real writes before any deadline checkpoint can
/// fire.
struct OpPathSlowResolver<R> {
    inner: R,
    delay: Duration,
    slow_table: &'static str,
    slow_table_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl<R: TableResolver + Send + Sync> TableResolver for OpPathSlowResolver<R> {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        if table_ref.table == self.slow_table
            && self.slow_table_calls.fetch_add(1, Ordering::SeqCst) >= 1
        {
            tokio::time::sleep(self.delay).await;
        }
        self.inner.resolve(table_ref).await
    }

    async fn resolve_repo(&self, repo_name: &str) -> DbResult<RepoInstance> {
        self.inner.resolve_repo(repo_name).await
    }
}

/// Plain repo-backed resolver for the transactional tests below (every
/// table lives in the single `RepoInstance`).
struct RepoResolver {
    repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for RepoResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

/// Count the rows currently committed in `table` (fresh scan).
async fn count_rows(repo: &RepoInstance, table: &str) -> usize {
    let tbl = repo.get_table(table).await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    count
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
    // `ExecutionDeadline::from_budget_secs` means the smallest real budget
    // is 1 real second. The slow mock's delay must genuinely exceed that so
    // the deadline is deterministically (not racily) expired by the time
    // the next cooperative checkpoint runs — 1.3s gives comfortable margin
    // without making the test unreasonably slow.
    let slow = SlowResolver {
        inner: resolver,
        delay: Duration::from_millis(1300),
        calls: AtomicUsize::new(0),
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
// Test 4b — a huge client-supplied budget (the `QueryLimitsCap::UNLIMITED`
// sentinel is `u64::MAX`, reachable whenever no operator cap clamps it, e.g.
// the embedded/napi `execute_batch` path) must not panic. `Instant`'s
// `Add<Duration>` panics on overflow; `ExecutionDeadline::from_budget_secs`
// must use `checked_add` and fall back to an effectively-unbounded deadline
// instead — a budget too large to represent can never actually elapse, so
// treating it as unbounded is behaviorally correct, not just panic-safe.
// ============================================================================

#[test]
fn execution_deadline_from_absurd_budget_does_not_panic() {
    use crate::query::batch::ExecutionDeadline;

    // Must not panic constructing the deadline...
    let deadline = ExecutionDeadline::from_budget_secs(u64::MAX);
    // ...and a normal (non-expired) checkpoint must still pass.
    assert!(
        deadline.check().is_ok(),
        "a checked_add overflow must fall back to an unbounded deadline, \
         not a deadline that is somehow already expired"
    );
}

#[tokio::test]
async fn execute_batch_with_absurd_budget_succeeds_without_panicking() {
    let resolver = setup().await;

    let mut req = insert_body_of("orders", "user_id", 3);
    req.limits = BatchLimits {
        max_execution_time_secs: u64::MAX, // the QueryLimitsCap::UNLIMITED sentinel
        ..BatchLimits::default()
    };

    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .expect("a u64::MAX budget must not panic and must not spuriously time out");
    assert!(
        resp.results.get("ins").is_some(),
        "the insert must have actually run and produced a result"
    );
}

// ============================================================================
// Test 5 — DECISIVE: a deadline that expires PARTWAY through a transactional
// `ForEach` stops the loop at the next per-iteration checkpoint, surfaces
// `ExecutionTimedOut` through the normal `Err` path, and rolls back the
// iterations that HAD already staged real writes in the outer tx.
// ============================================================================

/// A TRANSACTIONAL outer batch whose single entry is a `ForEach` over 8
/// elements; the body inserts one `users` row per iteration, and every
/// `resolve()` is delayed 300 ms (2 resolves per iteration — the body's
/// `validate_tables` + the op dispatch — ≈600 ms per iteration against a
/// 1 s budget). Because the outer batch is transactional, every iteration's
/// insert is staged in the SAME outer `TxContext` (#661 threading via
/// `run_nested_body_in_outer_tx`).
///
/// Iteration 0 provably completes (checkpoint before it runs at ≈0 s; its
/// two 300 ms delays finish well inside the budget) and stages a real,
/// uncommitted row. By iteration 1–2 the budget has elapsed, so the NEXT
/// cooperative checkpoint — the per-iteration `deadline.check()` in
/// `QueryRunner::run`'s `ForEach` loop (or the per-alias check inside the
/// nested body) — returns `Err(ExecutionTimedOut)`, which propagates
/// UNWRAPPED through the `for_each` error path into
/// `execute_transactional_impl`'s existing `Err` arm: pessimistic locks
/// released (no-op here — Snapshot), `commit_tx` never called, `TxContext`
/// dropped = RAII rollback of every staged row.
///
/// **Why this test fails against the reverted (no-mechanism) code**: with
/// no deadline anywhere, all 8 iterations simply run to completion (~4.8 s),
/// the tx COMMITS, `execute_batch` returns `Ok` — the `expect_err` fails,
/// and even the rollback assertion would see 8 committed rows. And unlike
/// this test's predecessor (which delayed the up-front `validate_tables`
/// resolves so the budget elapsed BEFORE `begin_tx` — making "no partial
/// rows survive" pass vacuously with zero writes ever staged), the
/// `calls >= 2` assertion below PROVES iteration 0's op dispatch ran inside
/// the open tx: past the nested per-alias checkpoint nothing can interrupt
/// the insert, so a staged write really existed and really was rolled back.
#[tokio::test]
async fn execute_batch_timeout_during_transactional_batch_rolls_back_partial_writes() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();

    let slow = SlowResolver {
        inner: RepoResolver { repo: repo.clone() },
        delay: Duration::from_millis(300),
        calls: AtomicUsize::new(0),
    };

    // Loop body: one insert into `users` per iteration. Non-transactional
    // body — the only reachable shape inside a transactional outer batch
    // (the `nested_tx_not_supported` guard) — whose writes join the OUTER
    // tx via #661's tx threading.
    let body = insert_body_of("users", "staged", 1);
    let fe = ForEachOp {
        over: FilterValue::Array((0..8i64).map(FilterValue::Int).collect()),
        bind_row: "uid".to_string(),
        batch: body,
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
        id: QueryValue::Int(666),
        name: None,
        transactional: true, // atomic — completed iterations must roll back too
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
            "a transactional ForEach at ~600ms/iteration against a 1s \
             budget must hit a deadline checkpoint partway through the loop",
        );
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 1 }),
        "expected ExecutionTimedOut{{budget_secs:1}}, got {:?}",
        err
    );

    // (b) NON-VACUITY GUARD: iteration 0 provably dispatched its insert
    // inside the open tx before any checkpoint could fire — its body made
    // BOTH resolve() calls (validate + op dispatch). Past the op-dispatch
    // resolve there is no further checkpoint before the row is staged, so
    // a real uncommitted write existed when the deadline fired.
    let calls = slow.calls.load(Ordering::SeqCst);
    assert!(
        calls >= 2,
        "iteration 0 must have reached its op dispatch (>= 2 resolve calls) \
         before the deadline fired — got {} call(s); if this fails the test \
         has gone vacuous again (timed out before staging any write)",
        calls
    );

    // (c) a FRESH read afterward shows NO staged row survived — the
    // deadline error flowed through `execute_transactional_impl`'s `Err`
    // arm, `commit_tx` was never called, and dropping the tx rolled back
    // every completed iteration's insert.
    let count = count_rows(&repo, "users").await;
    assert_eq!(
        count, 0,
        "iteration 0's staged insert must be rolled back with the rest of \
         the tx once the deadline checkpoint aborts the batch — found {} \
         row(s); against the reverted code the loop would have run to \
         completion and committed all 8 rows",
        count
    );
}

// ============================================================================
// Test 6 — the deadline is consulted one final time immediately BEFORE
// `commit_tx`: a budget that elapses while the LAST op executes aborts the
// batch through the normal `Err` path, and `commit_tx` is never entered.
// ============================================================================

/// Single-op transactional batch whose one insert stages successfully but
/// SLOWLY (the op-dispatch `resolve()` sleeps 1.3 s against a 1 s budget;
/// the up-front validation resolve is left fast so the tx genuinely opens
/// and the op genuinely runs). The plan therefore returns `Ok` with the
/// budget already blown — exactly the shape where the OLD preemptive
/// `tokio::time::timeout` could cancel the in-flight `commit_tx` between
/// its WAL-begin and completion, leaving the tx DURABLY COMMITTED while
/// reporting `Err(ExecutionTimedOut)` (the WAL/in-memory divergence bug).
///
/// Under the cooperative redesign the pre-commit checkpoint in
/// `execute_transactional_impl` folds the expired deadline into
/// `plan_result` BEFORE the commit decision: the existing `Err` arm runs
/// (locks released, `commit_tx` never called), the tx drops = RAII
/// rollback. Structurally, a single-op batch can now ONLY observe the
/// deadline before the op or before commit — never mid-commit, because no
/// cancellation source exists anywhere in `execute_batch` anymore.
///
/// The `Err(ExecutionTimedOut)` + zero-surviving-rows pair below is the
/// brief's sanctioned proxy for "`commit_tx` was never entered": had
/// commit begun, the batch would either have returned `Ok` (commit
/// observed) or — under the old racy code — committed durably while
/// erroring, which the fresh-read assertion would expose after recovery.
#[tokio::test]
async fn deadline_expired_before_commit_aborts_via_err_path_without_committing() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory("test".into(), factory, vec![TableConfig::new("users")])
        .await
        .unwrap();

    let slow = OpPathSlowResolver {
        inner: RepoResolver { repo: repo.clone() },
        delay: Duration::from_millis(1300),
        slow_table: "users",
        slow_table_calls: AtomicUsize::new(0),
    };

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![mpack!({ "name": "must_not_commit" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let req = BatchRequest {
        id: QueryValue::Int(667),
        name: None,
        transactional: true,
        isolation: None,
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits {
            max_execution_time_secs: 1,
            ..BatchLimits::default()
        },
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    let err = execute_batch(&req, &slow, None, None, Actor::System, "test")
        .await
        .expect_err(
            "the op stages in 1.3s against a 1s budget, so the pre-commit \
             deadline checkpoint must abort the batch",
        );
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 1 }),
        "expected ExecutionTimedOut{{budget_secs:1}}, got {:?}",
        err
    );

    // The op-dispatch resolve DID run (the insert was staged; the plan
    // returned Ok) — so the abort decision was taken at the PRE-COMMIT
    // checkpoint, not before the op.
    assert!(
        slow.slow_table_calls.load(Ordering::SeqCst) >= 2,
        "the op-dispatch resolve must have run — otherwise this test is \
         not exercising the pre-commit checkpoint"
    );

    // Nothing committed: the staged insert was rolled back when the tx
    // dropped on the Err path. `commit_tx` was never entered.
    let count = count_rows(&repo, "users").await;
    assert_eq!(
        count, 0,
        "the staged insert must NOT survive — the pre-commit deadline \
         checkpoint must abort via the Err arm without ever calling \
         commit_tx; found {} row(s)",
        count
    );
}

// ============================================================================
// Test 7 — finding-2 regression: a PESSIMISTIC (Level-3) batch that times
// out at a mid-plan checkpoint releases its already-acquired locks through
// the existing `Err`-arm `release_pessimistic_locks` — a subsequent tx on
// the same key proceeds promptly instead of hanging in wound-wait.
// ============================================================================

/// Three-stage `pessimistic`-isolation transactional batch:
///   1. `"lock"`  — UPDATE `users` WHERE k==1: acquires the Level-3
///      Exclusive lock on the seeded row and stages the write (fast).
///   2. `"slow"`  — INSERT into `orders`, whose op-dispatch `resolve()`
///      sleeps 1.3 s (validation resolve left fast), blowing the 1 s budget.
///   3. `"never"` — the per-alias deadline checkpoint fires BEFORE this op,
///      raising `Err(ExecutionTimedOut)` out of `execute_plan_tx_impl`.
///
/// That `Err` lands in `execute_transactional_impl`'s existing `Err` arm,
/// whose FIRST action is `release_pessimistic_locks(&tx, &repo)` — the
/// exact call site the old preemptive future-drop bypassed (dropping the
/// future mid-plan skipped the `match plan_result` entirely, and
/// `TxContext` has no `Drop` impl for MvccStore locks, so the Exclusive
/// lock on the `users` row leaked PERMANENTLY: under wound-wait a younger
/// tx waits unboundedly for an older holder that no longer exists).
///
/// The probe batch then re-locks the SAME row from a fresh (younger)
/// Level-3 tx, bounded by a 5 s harness timeout: against the old code it
/// hangs (leaked lock) and the bound fails the test with a clear message;
/// under the cooperative redesign it commits promptly.
#[tokio::test]
async fn pessimistic_batch_timeout_releases_level3_locks_for_subsequent_tx() {
    let factory = BoxRepoFactory::in_memory();
    let repo = RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![TableConfig::new("users"), TableConfig::new("orders")],
    )
    .await
    .unwrap();
    let plain = RepoResolver { repo: repo.clone() };

    // Seed the contended row (committed outside the batches under test).
    let mut seed_queries = new_map();
    seed_queries.insert(
        "seed".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![mpack!({ "k": 1, "v": 0 })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: false,
            after: Vec::new(),
            when: None,
        },
    );
    let seed_req = BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        durability: None,
        queries: seed_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    execute_batch(&seed_req, &plain, None, None, Actor::System, "test")
        .await
        .expect("seeding the contended row must succeed");

    // The timing-out pessimistic batch: lock → slow → never.
    let slow = OpPathSlowResolver {
        inner: RepoResolver { repo: repo.clone() },
        delay: Duration::from_millis(1300),
        slow_table: "orders",
        slow_table_calls: AtomicUsize::new(0),
    };
    let mut queries = new_map();
    queries.insert(
        "lock".to_string(),
        QueryEntry {
            op: BatchOp::Update(
                write::update("users")
                    .set(doc().set("v", 10_i64))
                    .where_(filter::eq("k", 1_i64))
                    .build(),
            ),
            return_result: false,
            after: Vec::new(),
            when: None,
        },
    );
    queries.insert(
        "slow".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![mpack!({ "note": "blows_the_budget" })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: false,
            after: vec!["lock".to_string()],
            when: None,
        },
    );
    queries.insert(
        "never".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("users"),
                values: vec![mpack!({ "k": 99 })],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: false,
            after: vec!["slow".to_string()],
            when: None,
        },
    );
    let req = BatchRequest {
        id: QueryValue::Int(668),
        name: None,
        transactional: true,
        isolation: Some("pessimistic".to_string()),
        durability: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits {
            max_execution_time_secs: 1,
            ..BatchLimits::default()
        },
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };

    // (a) the batch errors with ExecutionTimedOut at the checkpoint before
    // "never".
    let err = execute_batch(&req, &slow, None, None, Actor::System, "test")
        .await
        .expect_err("the 1.3s op against a 1s budget must trip the per-alias checkpoint");
    assert!(
        matches!(err, BatchError::ExecutionTimedOut { budget_secs: 1 }),
        "expected ExecutionTimedOut{{budget_secs:1}}, got {:?}",
        err
    );

    // (b) rollback really happened — neither the staged update's effect nor
    // the orders insert survives.
    assert_eq!(
        count_rows(&repo, "orders").await,
        0,
        "the timed-out batch's staged orders insert must be rolled back"
    );

    // (c) THE finding-2 assertion: a fresh, otherwise-unrelated Level-3 tx
    // needing the SAME lock the timed-out batch held must proceed promptly.
    // Bounded so a leaked lock FAILS loudly instead of hanging the suite —
    // under wound-wait this younger tx would otherwise wait forever on the
    // dead batch's Exclusive lock.
    let mut probe_queries = new_map();
    probe_queries.insert(
        "upd".to_string(),
        QueryEntry {
            op: BatchOp::Update(
                write::update("users")
                    .set(doc().set("v", 99_i64))
                    .where_(filter::eq("k", 1_i64))
                    .build(),
            ),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let probe_req = BatchRequest {
        id: QueryValue::Int(669),
        name: None,
        transactional: true,
        isolation: Some("pessimistic".to_string()),
        durability: None,
        queries: probe_queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    };
    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        execute_batch(&probe_req, &plain, None, None, Actor::System, "test"),
    )
    .await
    .expect(
        "LOCK LEAK: a fresh Level-3 tx hung acquiring the lock the \
         timed-out batch held — release_pessimistic_locks did not run on \
         the timeout path",
    )
    .expect("the probe batch must succeed once the lock is free");
    let tx_info = resp
        .transaction
        .expect("transactional probe must carry TransactionInfo");
    assert_eq!(
        tx_info.status, "committed",
        "the probe tx must commit cleanly on the released lock, got {:?} \
         (reason: {:?})",
        tx_info.status, tx_info.reason
    );
}
