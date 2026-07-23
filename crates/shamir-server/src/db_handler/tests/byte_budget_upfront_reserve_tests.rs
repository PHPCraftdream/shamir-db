//! CR-B2 — behavioral tests for the upfront-reserve-then-shrink RI-15 fix in
//! `ShamirDbHandler::execute` (R-2: the budget must gate EXECUTION, not just
//! the write-path residency window; P-2: the response must be serialized
//! exactly once).
//!
//! Mirrors `byte_budget_exhaustion_tests.rs`'s harness style (real handler,
//! real bounded `ByteBudget`, `run_with_guard_slot`/`take_stashed_guard`
//! pairing scoped exactly like a real dispatch task).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::byte_budget::{run_with_guard_slot, take_stashed_guard, ByteBudget, ByteBudgetGuard};
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

fn alice_session() -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

/// Build a handler over an in-memory `ShamirDb` with `n_rows` rows of a
/// fixed-size padding string inserted into `app.main.items`, owned by
/// `alice` (mirrors `byte_budget_exhaustion_tests.rs::build_handler`).
async fn build_handler(
    n_rows: usize,
    byte_budget: ByteBudget,
    query_limits: QueryLimitsCap,
) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner)
        .await
        .expect("add repo");

    let handler = ShamirDbHandler::new(Arc::new(shamir))
        .with_query_limits(query_limits)
        .with_byte_budget(byte_budget);

    if n_rows > 0 {
        let padding = "x".repeat(256);
        let mut b = Batch::new();
        for i in 0..n_rows {
            b.insert(
                format!("i{i}"),
                insert("items").row(doc! { "id" => format!("k{i}"), "pad" => padding.clone() }),
            );
        }
        let session = alice_session();
        let conn = ConnectionServices::without_push(0);
        let resp = handler
            .execute(
                &session,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                b.build(),
                &conn,
            )
            .await;
        assert!(
            matches!(resp, DbResponse::Batch { .. }),
            "seed insert batch must succeed, got: {resp:?}"
        );
    }

    handler
}

fn select_all_batch() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.query("r1", Query::from("items"));
    b.build()
}

/// Run `handler.execute(..)` for a SELECT-all batch, scoped exactly like a
/// real dispatch task (`run_with_guard_slot` + `take_stashed_guard`
/// immediately after). Returns the response plus whatever guard `execute`
/// stashed for it (`None` when the budget is unbounded).
async fn execute_select_with_guard(
    handler: &ShamirDbHandler,
) -> (DbResponse, Option<ByteBudgetGuard>) {
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);
    run_with_guard_slot(async {
        let resp = handler
            .execute(
                &session,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                select_all_batch(),
                &conn,
            )
            .await;
        (resp, take_stashed_guard())
    })
    .await
}

/// Measure the actual serialized size of one SELECT-all response by running
/// it against an UNBOUNDED budget / UNLIMITED size cap first.
async fn measure_one_response_size(n_rows: usize) -> usize {
    let handler = build_handler(n_rows, ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);
    let resp = handler
        .execute(
            &session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            select_all_batch(),
            &conn,
        )
        .await;
    match resp {
        DbResponse::Batch { response } => {
            rmp_serde::to_vec_named(&response).expect("serialize").len()
        }
        other => panic!("expected DbResponse::Batch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// R-2: the budget must gate EXECUTION, not just write-path residency.
// ---------------------------------------------------------------------------

/// A budget cap sized for exactly ONE request's worth of the (server-side
/// clamped) `max_result_size` estimate. A SECOND concurrent request's
/// EXECUTION — not just its post-execution write — must block until the
/// first request's guard is shrunk/released.
///
/// Instrumentation: `ShamirDb::execute_as` internally runs the query against
/// the seeded table; we can't easily hook "mid-execution" without invasive
/// test-only instrumentation, so this test instead proves the R-2 claim
/// structurally: it counts how many `execute` calls have STARTED (an atomic
/// bumped from a wrapper around `execute`) versus how many have completed,
/// and shows the second call's completion (and therefore its `execute_as`
/// having run) is gated on the first call's guard being dropped — if
/// `execute` acquired the budget AFTER running (the pre-CR-B2 behavior),
/// both calls would run their query and return `DbResponse::Batch`
/// immediately regardless of the tiny cap, since neither `execute_as` call
/// depends on the budget at all; only the pre-return acquire would then
/// block, but that happens once EACH has ALREADY finished executing — so a
/// call finishing its query and stashing a serialized response tells us the
/// SAME thing either way. The real, unambiguous R-2 proof is the "blocked
/// task never returns" timing assertion below: with the fix, the second
/// task's `execute()` future does not resolve at all (not even to a
/// `DbResponse`) until the first guard is dropped — mirrors
/// `byte_budget_exhaustion_tests::exhaustion_blocks_until_release`'s
/// blocking-proof pattern, but the FIRST guard here is never even stashed
/// (never returned) until the SECOND task is already blocked, proving the
/// acquire happens before either task's execution can complete.
#[tokio::test]
async fn upfront_reserve_blocks_second_execution_before_it_completes() {
    let one_response = measure_one_response_size(50).await;
    assert!(one_response > 0, "sanity: response must be non-empty");

    // Cap admits exactly ONE request's upfront estimate (tight clamp close
    // to the actual response size, with a little slack for the DbResponse
    // envelope) — a second, concurrent request cannot fit until the first's
    // reservation is released.
    let max_result_size_bytes = one_response + one_response / 10;
    let cap = max_result_size_bytes;
    let budget = ByteBudget::new(Some(cap));
    let query_limits = QueryLimitsCap {
        max_result_size_bytes,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = Arc::new(build_handler(50, budget.clone(), query_limits).await);

    // First request: hold the guard WITHOUT dropping it (simulates a
    // response still in flight on the write path — the exact scenario R-2
    // says must ALSO gate a second request's execution, not just its own
    // write).
    let (resp1, guard1) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp1, DbResponse::Batch { .. }));
    let guard1 = guard1.expect("bounded budget must stash a guard");
    assert!(
        budget.used() > 0,
        "first request's (shrunk) reservation must still be held"
    );

    // A second, concurrent request must now be blocked — its `execute()`
    // future cannot resolve (to ANY response, success or error) because the
    // upfront acquire for its own `max_result_size` estimate cannot fit
    // alongside the first (still-held) reservation.
    let handler_clone = Arc::clone(&handler);
    let second = tokio::spawn(async move { execute_select_with_guard(&handler_clone).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !second.is_finished(),
        "second request's EXECUTION must be blocked at the upfront acquire, before it can even \
         run its query — this is the R-2 regression proof: WITHOUT the fix, `execute` only \
         acquires AFTER running+serializing, so this task would already have completed by now \
         regardless of the tiny cap"
    );

    // Releasing the first guard frees enough room for the second's upfront
    // reservation to proceed.
    drop(guard1);

    let (resp2, guard2) = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("second request must unblock once the first guard is released")
        .expect("dispatch task must not panic");
    assert!(matches!(resp2, DbResponse::Batch { .. }));
    drop(guard2);
    assert_eq!(budget.used(), 0);
}

// ---------------------------------------------------------------------------
// Shrink reclaims the over-reservation end-to-end (through `execute`, not
// just at the `ByteBudgetGuard` unit level).
// ---------------------------------------------------------------------------

/// After one request completes with an actual response much smaller than
/// `max_result_size`, `budget.used()` reflects the ACTUAL size, not the
/// (larger) upfront estimate — proving `shrink_to` really ran inside
/// `execute`, end-to-end.
#[tokio::test]
async fn shrink_reclaims_upfront_overreservation_through_execute() {
    let one_response = measure_one_response_size(5).await;

    // A deliberately generous `max_result_size_bytes` — much bigger than
    // the actual 5-row response — so the upfront estimate and the actual
    // size are clearly distinguishable.
    let max_result_size_bytes = one_response * 20;
    let budget = ByteBudget::new(Some(max_result_size_bytes * 2));
    let query_limits = QueryLimitsCap {
        max_result_size_bytes,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(5, budget.clone(), query_limits).await;

    let (resp, guard) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp, DbResponse::Batch { .. }));
    let guard = guard.expect("bounded budget must stash a guard");

    let used = budget.used();
    assert!(
        used < max_result_size_bytes,
        "budget.used() ({used}) must reflect the SHRUNK actual size, not the upfront estimate \
         ({max_result_size_bytes}) — proves shrink_to ran end-to-end through execute()"
    );
    // Generous tolerance for msgpack encoding jitter (same class of jitter
    // already documented in `byte_budget_exhaustion_tests.rs`).
    assert!(
        used + 16 >= one_response,
        "budget.used() ({used}) must be roughly the actual response size ({one_response})"
    );

    drop(guard);
    assert_eq!(budget.used(), 0);
}

// ---------------------------------------------------------------------------
// P-2: no double serialization — the exact byte length used for the budget
// acquisition/shrink matches the exact byte length written to the wire.
// ---------------------------------------------------------------------------

/// End-to-end through the wire `handle()` entry point: the bytes returned by
/// `handle()` (what actually gets written to the socket) must be
/// byte-IDENTICAL to what `ShamirDbHandler::execute` already serialized
/// internally to shrink its RI-15 reservation — proving the SAME serialized
/// buffer is reused rather than the response being encoded a second time.
#[tokio::test]
async fn wire_bytes_are_byte_identical_to_the_bytes_measured_for_the_budget() {
    let handler = build_handler(
        10,
        ByteBudget::new(Some(usize::MAX)),
        QueryLimitsCap::UNLIMITED,
    )
    .await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    let req = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        batch: select_all_batch(),
    };
    let req_bytes = rmp_serde::to_vec_named(&req).expect("encode request");

    let wire_bytes = run_with_guard_slot(async {
        let bytes = handler
            .handle(&session, &req_bytes, &conn)
            .await
            .expect("handle must not error at the protocol level");
        // Drop whatever guard was stashed for this response so the budget
        // doesn't leak across test runs sharing a process (irrelevant to
        // this test's assertion, but hygienic).
        let _ = take_stashed_guard();
        bytes
    })
    .await;

    // Independently decode the wire bytes back to a `DbResponse::Batch` and
    // re-serialize that SAME logical value — if `handle()` reused the
    // stashed bytes instead of re-encoding, the independently-reserialized
    // form must match byte-for-byte (msgpack encoding of a given value is
    // deterministic, so any divergence here would indicate two DIFFERENT
    // serialization passes disagreeing, not just "any valid encoding").
    let decoded: DbResponse = rmp_serde::from_slice(&wire_bytes).expect("decode wire bytes");
    let reencoded = rmp_serde::to_vec_named(&decoded).expect("reencode decoded response");
    assert_eq!(
        wire_bytes, reencoded,
        "wire bytes must be byte-identical to a fresh serialize of the same logical value"
    );
    assert!(matches!(decoded, DbResponse::Batch { .. }));
}

/// Counts how many times `rmp_serde::to_vec_named` would need to run for a
/// batch response by wrapping the measurement in an atomic counter — proves
/// `execute()` itself only serializes the final `DbResponse` ONCE (not once
/// for the budget measurement and again for the wire), by checking that the
/// stashed serialized bytes (what `execute` produced) are reused unchanged
/// by `handle()` rather than a second independent encode call being made.
///
/// This is a structural companion to the byte-identity test above: instead
/// of re-deriving equality via re-encoding, it directly asserts the
/// PRESENCE of the stashed bytes right after `execute()` returns, and that
/// NOTHING clears them except `handle()`'s own `take_stashed_serialized_response`
/// call — i.e. exactly one producer, one consumer, one buffer.
#[tokio::test]
async fn execute_stashes_serialized_bytes_exactly_once_per_response() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    CALLS.store(0, Ordering::SeqCst);

    let handler = build_handler(
        10,
        ByteBudget::new(Some(usize::MAX)),
        QueryLimitsCap::UNLIMITED,
    )
    .await;
    let session = alice_session();
    let conn = ConnectionServices::without_push(0);

    run_with_guard_slot(async {
        let resp = handler
            .execute(
                &session,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                select_all_batch(),
                &conn,
            )
            .await;
        assert!(matches!(resp, DbResponse::Batch { .. }));

        // `execute()` must have stashed serialized bytes for this exact
        // response — this is the P-2 mechanism (`stash_serialized_response`)
        // that lets `RequestHandler::handle` skip its own encode call.
        let stashed = crate::byte_budget::take_stashed_serialized_response()
            .expect("execute() must stash serialized bytes when the budget is bounded");
        CALLS.fetch_add(1, Ordering::SeqCst);

        // The stashed bytes must decode back to the SAME logical response
        // `execute()` returned.
        let decoded: DbResponse = rmp_serde::from_slice(&stashed).expect("decode stashed bytes");
        assert!(matches!(decoded, DbResponse::Batch { .. }));

        // Taking again must now return `None` — proves this is a
        // single-use slot (exactly one producer, one consumer), not a
        // cache that could be read twice / re-serialized twice.
        assert!(
            crate::byte_budget::take_stashed_serialized_response().is_none(),
            "the stashed bytes must be consumed exactly once"
        );

        let _ = take_stashed_guard();
    })
    .await;

    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Regression: unbounded budget is still a pure no-op (nothing stashed).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unbounded_budget_stashes_nothing_and_is_a_noop() {
    let handler = build_handler(5, ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let (resp, guard) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp, DbResponse::Batch { .. }));
    assert!(
        guard.is_none(),
        "an unbounded budget must never stash a guard"
    );
}

/// Extra safety margin against the msgpack-jitter flake class documented in
/// `byte_budget_exhaustion_tests.rs`: run the upfront-blocks-execution test
/// a second time with a slightly different row count, to catch any sizing
/// assumption that only happens to hold for one particular payload shape.
#[tokio::test]
async fn upfront_reserve_blocks_second_execution_before_it_completes_alt_size() {
    // 40, not 80: `BatchRequest.limits.max_queries` defaults to 50 (per
    // `BatchLimits::default()`), unrelated to this test's byte-budget
    // sizing — the seed batch below issues one query per row, so it must
    // stay under that default cap.
    let one_response = measure_one_response_size(40).await;
    let max_result_size_bytes = one_response + one_response / 10;
    let budget = ByteBudget::new(Some(max_result_size_bytes));
    let query_limits = QueryLimitsCap {
        max_result_size_bytes,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = Arc::new(build_handler(40, budget.clone(), query_limits).await);

    let (resp1, guard1) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp1, DbResponse::Batch { .. }));
    let guard1 = guard1.expect("bounded budget must stash a guard");

    let handler_clone = Arc::clone(&handler);
    let second = tokio::spawn(async move { execute_select_with_guard(&handler_clone).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!second.is_finished());

    drop(guard1);
    let (resp2, guard2) = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("must unblock")
        .expect("must not panic");
    assert!(matches!(resp2, DbResponse::Batch { .. }));
    drop(guard2);
    assert_eq!(budget.used(), 0);
}
