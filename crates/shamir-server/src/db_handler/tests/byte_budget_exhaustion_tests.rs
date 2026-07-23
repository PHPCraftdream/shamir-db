//! Behavioral tests for the RI-15 global in-flight response-byte budget as
//! wired through `ShamirDbHandler::execute`.
//!
//! `execute` stashes the acquired [`crate::byte_budget::ByteBudgetGuard`] in
//! a task-local (`crate::byte_budget::stash_guard`) rather than returning
//! it directly (its return type is `DbResponse`, asserted on by
//! `node_mode_tests.rs`). These tests reproduce the same
//! `run_with_guard_slot` / `take_stashed_guard` pairing that
//! `connection::request_loop` uses in production, so each simulated
//! "dispatch task" here is a `tokio::spawn`ed future scoped exactly like a
//! real one.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;

use crate::byte_budget::{run_with_guard_slot, take_stashed_guard, ByteBudget, ByteBudgetGuard};
use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
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
/// `alice` (mirrors `node_mode_tests.rs::build_handler`). `byte_budget` and
/// `query_limits` are installed on the handler as-is (caller controls
/// whether either gate is active).
///
/// CR-B2: `query_limits` matters even for tests that only care about the
/// RI-15 budget — `ShamirDbHandler::execute` now reserves
/// `batch.limits.max_result_size` (clamped to
/// `query_limits.max_result_size_bytes`) UPFRONT, before execution. Passing
/// `QueryLimitsCap::UNLIMITED` here would leave that clamp at the client's
/// raw default (`BatchLimits::default().max_result_size` = 10 MiB), an
/// unrealistic upfront estimate against a tiny test-sized budget cap — mirrors
/// production, where config-load validation already requires
/// `max_inflight_response_bytes >= max_result_size_bytes`, so a bounded RI-15
/// budget always implies a bounded per-batch cap in practice.
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

    // Seed rows through the same handler path (execute) so table state is
    // identical to how a real client would populate it.
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

/// Measure the actual serialized size of one SELECT-all response by
/// running it against an UNBOUNDED budget / UNLIMITED size cap first — this
/// is what a production cap would be sized relative to, and keeps the test
/// independent of the exact msgpack encoding overhead.
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

/// N concurrent max-size-response batches saturate the budget; the
/// (N+1)-th blocks until one of the first N's guard is released — mirroring
/// the production sequence where the WRITER task (not the dispatch task)
/// drops the guard after the socket write completes.
#[tokio::test]
async fn exhaustion_blocks_until_release() {
    let one_response = measure_one_response_size(50).await;
    assert!(one_response > 0, "sanity: response must be non-empty");

    // Cap admits exactly 2 concurrent max-size responses (with a little
    // slack so encoding jitter across identical queries can't flake it).
    let cap = one_response * 2 + one_response / 2;
    let budget = ByteBudget::new(Some(cap));
    // CR-B2: `execute` now reserves `batch.limits.max_result_size` (clamped
    // to this cap) UPFRONT, before execution, and only shrinks it down to
    // the actual size afterward. `max_result_size_bytes` is set TIGHT here
    // (close to `one_response`, not a loose multiple of it) so the upfront
    // estimate closely tracks the actual size — since request 1 and 2 below
    // run sequentially and request 1's guard is still HELD (not dropped)
    // when request 2's upfront reserve runs, the cap must accommodate
    // "request 1's shrunk actual size + request 2's upfront estimate", not
    // "2x the actual size" as it would under the old post-hoc-only
    // accounting. A loose upfront cap (e.g. `one_response * 2`) would make
    // request 2's upfront reserve alone nearly saturate this test's cap
    // on top of request 1's already-held actual bytes. Mirrors production's
    // config-load invariant that the RI-15 cap is always >=
    // `max_result_size_bytes`; without ANY realistic cap here the upfront
    // estimate would fall back to the client's raw 10 MiB default, which
    // could never fit this tiny test-sized budget at all.
    let query_limits = QueryLimitsCap {
        max_result_size_bytes: one_response + one_response / 10,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = Arc::new(build_handler(50, budget.clone(), query_limits).await);

    // Two "in-flight responses" saturate the budget.
    let (resp1, guard1) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp1, DbResponse::Batch { .. }));
    let guard1 = guard1.expect("bounded budget must stash a guard");

    let (resp2, guard2) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp2, DbResponse::Batch { .. }));
    let guard2 = guard2.expect("bounded budget must stash a guard");

    // Tolerance: `one_response` was measured from a SEPARATE execution
    // (`measure_one_response_size`); a response's serialized size can vary
    // by a few bytes call-to-call (e.g. a timing-derived stats field
    // crossing a msgpack varint width boundary), observed in CI as an
    // off-by-1..2-byte mismatch on macOS/Windows runners. The invariant
    // under test is "the budget accumulated roughly 2x, not just 1x" — a
    // 16-byte tolerance is generous against encoding jitter while still
    // catching a real accounting bug (which would be off by a whole
    // response's worth of bytes, not a handful).
    let used = budget.used();
    assert!(
        used + 16 >= one_response * 2,
        "two in-flight responses must reserve at least ~2x one response's bytes (used={}, one={})",
        used,
        one_response,
    );

    // A third concurrent response cannot fit — it must block.
    let handler_clone = Arc::clone(&handler);
    let third = tokio::spawn(async move { execute_select_with_guard(&handler_clone).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !third.is_finished(),
        "third response must be blocked while the budget is saturated by two guards"
    );

    // Release one of the first two — the third must now be able to
    // proceed (this simulates the writer task finishing its socket write
    // and dropping the guard).
    drop(guard1);

    let (resp3, guard3) = tokio::time::timeout(Duration::from_secs(5), third)
        .await
        .expect("third response must unblock after a release")
        .expect("dispatch task must not panic");
    assert!(matches!(resp3, DbResponse::Batch { .. }));
    let _guard3 = guard3.expect("bounded budget must stash a guard");

    drop(guard2);
    drop(_guard3);
    assert_eq!(
        budget.used(),
        0,
        "every guard released; budget must be fully drained"
    );
}

/// Release path on write error (simulated closed socket / write failure):
/// dropping the guard WITHOUT ever performing a successful write (i.e. the
/// writer task's error branch) must still return the reserved bytes to the
/// budget — the budget cannot leak just because the socket write failed.
#[tokio::test]
async fn release_on_simulated_write_error_recovers_budget() {
    let one_response = measure_one_response_size(50).await;
    let cap = one_response + one_response / 2;
    let budget = ByteBudget::new(Some(cap));
    // CR-B2: see `exhaustion_blocks_until_release`'s comment — a realistic
    // `max_result_size_bytes` is required so the upfront reservation
    // (`batch.limits.max_result_size` clamped to this cap) is meaningful
    // relative to this test's tiny budget cap.
    let query_limits = QueryLimitsCap {
        max_result_size_bytes: cap,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(50, budget.clone(), query_limits).await;

    let (resp, guard) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp, DbResponse::Batch { .. }));
    let guard = guard.expect("bounded budget must stash a guard");
    assert!(budget.used() >= one_response);

    // Simulate the writer task's error path: `connection::request_loop`'s
    // writer drops `budget_guard` unconditionally after
    // `write_frame_prereserved` regardless of whether that call returned
    // `Err` (broken pipe / dead client) — reproduced here by dropping the
    // guard without any successful write ever having happened.
    drop(guard);

    assert_eq!(
        budget.used(),
        0,
        "a guard dropped on the write-error path must still release its bytes \
         (this is the WriterMsg::Reply/ReplyAndClose write-error branch in \
         connection::request_loop — release must not depend on write success)"
    );

    // Budget must be usable again immediately — no permanent leak.
    let (resp2, guard2) = execute_select_with_guard(&handler).await;
    assert!(matches!(resp2, DbResponse::Batch { .. }));
    drop(guard2);
}
