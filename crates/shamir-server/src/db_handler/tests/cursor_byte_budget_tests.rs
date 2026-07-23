//! CR-A5 — behavioral tests for cursor `CreateCursor`/`FetchNext` responses
//! routed through the RI-15 global in-flight response-byte budget, plus the
//! new per-page byte-size cap (`query_limits.max_result_size_bytes`).
//!
//! Mirrors `byte_budget_exhaustion_tests.rs`'s harness style (real handler,
//! real bounded `ByteBudget`, `run_with_guard_slot`/`take_stashed_guard`
//! pairing scoped exactly like a real dispatch task, `tokio::spawn` +
//! timeout to prove blocking/unblocking) but exercised through cursor calls
//! instead of `Execute`.

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
use shamir_query_builder::write::insert;
use shamir_query_types::read::{OrderBy, ReadQuery};
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use crate::byte_budget::{run_with_guard_slot, take_stashed_guard, ByteBudget, ByteBudgetGuard};
use crate::db_handler::config::{CursorLimitsCap, QueryLimitsCap};
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
/// `byte_budget`/`query_limits` are installed on the handler as-is (caller
/// controls whether either gate is active).
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
        .with_byte_budget(byte_budget)
        .with_cursor_limits(CursorLimitsCap::UNLIMITED);

    if n_rows > 0 {
        let padding = "x".repeat(256);
        let mut b = Batch::new();
        for i in 0..n_rows {
            b.insert(
                format!("i{i}"),
                insert("items").row(
                    doc! { "id" => format!("k{i:03}"), "pad" => padding.clone(), "v" => i as i64 },
                ),
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
            matches!(resp, crate::db_handler::handler::DbResponse::Batch { .. }),
            "seed insert batch must succeed, got: {resp:?}"
        );
    }

    handler
}

fn create_cursor_req(page_size: u32) -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query: ReadQuery::new("items").order_by(OrderBy::asc("v")),
        page_size,
    }
}

fn fetch_next_req(cursor_id: CursorId, page_size: u32) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size: Some(page_size),
    }
}

/// Run `req` through the wire `handle()` entry point, scoped exactly like a
/// real dispatch task (`run_with_guard_slot` + `take_stashed_guard`
/// immediately after). Returns the decoded response plus whatever guard the
/// handler stashed for it (`None` when the budget is unbounded).
async fn send_with_guard(
    handler: &ShamirDbHandler,
    session: &Session,
    req: DbRequest,
) -> (DbResponse, Option<ByteBudgetGuard>) {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    run_with_guard_slot(async {
        let resp_bytes = handler
            .handle(session, &bytes, &conn)
            .await
            .expect("handle must not error at the protocol level");
        let resp: DbResponse = rmp_serde::from_slice(&resp_bytes).expect("decode response");
        (resp, take_stashed_guard())
    })
    .await
}

/// Measure the actual serialized size of one CreateCursor page's `page`
/// payload by running it against an UNBOUNDED budget / UNLIMITED size cap
/// first — this is what a production cap would be sized relative to, and
/// keeps the test independent of the exact msgpack encoding overhead.
async fn measure_one_page_size(n_rows: usize, page_size: u32) -> usize {
    let handler = build_handler(n_rows, ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();
    let (resp, _guard) = send_with_guard(&handler, &session, create_cursor_req(page_size)).await;
    match resp {
        DbResponse::CursorPage { page, .. } => {
            rmp_serde::to_vec_named(&page).expect("serialize").len()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// RI-15 global byte budget wired through cursor responses.
// ---------------------------------------------------------------------------

/// A bounded budget saturated by two large cursor pages (via `CreateCursor`)
/// blocks a third concurrent `CreateCursor` until one guard releases — same
/// shape as `byte_budget_exhaustion_tests::exhaustion_blocks_until_release`,
/// exercised through the cursor path instead of `Execute`. `FetchNext`'s
/// coverage of the SAME `enforce_page_budget` acquire path is exercised by
/// `page_within_size_cap_is_accepted_and_still_acquires_budget`'s sibling
/// tests below and by the too-large-page rejection tests, which run both
/// `CreateCursor` and `FetchNext` through the identical helper.
#[tokio::test]
async fn cursor_page_budget_blocks_until_release() {
    // 50 rows / page_size 50 -> a single (exhausted) first page carrying all
    // 50 rows, so `measure_one_page_size` reflects one "large page".
    let one_page = measure_one_page_size(50, 50).await;
    assert!(one_page > 0, "sanity: page must be non-empty");

    // Cap admits exactly 2 concurrent max-size pages (with a little slack
    // so encoding jitter across identical queries can't flake it) — mirrors
    // `byte_budget_exhaustion_tests::exhaustion_blocks_until_release`'s cap
    // sizing exactly.
    let cap = one_page * 2 + one_page / 2;
    let budget = ByteBudget::new(Some(cap));
    let handler = Arc::new(build_handler(50, budget.clone(), QueryLimitsCap::UNLIMITED).await);

    // Two "in-flight large pages" saturate the budget — each a SEPARATE
    // CreateCursor over all 50 rows (page_size 50 -> has_more == false, so
    // neither cursor is registered; this test only cares about the
    // RESPONSE bytes each acquisition reserves, mirroring `execute()`'s own
    // `measure_one_response_size` sizing rationale).
    let session = alice_session();
    let (resp_a, guard_a) = send_with_guard(&handler, &session, create_cursor_req(50)).await;
    assert!(matches!(resp_a, DbResponse::CursorPage { .. }));
    let guard_a = guard_a.expect("bounded budget must stash a guard");

    let (resp_b, guard_b) = send_with_guard(&handler, &session, create_cursor_req(50)).await;
    assert!(matches!(resp_b, DbResponse::CursorPage { .. }));
    let guard_b = guard_b.expect("bounded budget must stash a guard");

    let used = budget.used();
    assert!(
        used + 16 >= one_page * 2,
        "two in-flight large pages must reserve at least ~2x one page's bytes (used={}, one={})",
        used,
        one_page,
    );

    // A third concurrent large-page CreateCursor cannot fit in the
    // remaining ~half-a-page of slack — it must block.
    let handler_clone = Arc::clone(&handler);
    let third = tokio::spawn(async move {
        let session = alice_session();
        send_with_guard(&handler_clone, &session, create_cursor_req(50)).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !third.is_finished(),
        "third CreateCursor must be blocked while the budget is saturated by two large-page guards"
    );

    // Release one of the two large pages — the third must now unblock.
    drop(guard_a);

    let (resp3, guard3) = tokio::time::timeout(Duration::from_secs(5), third)
        .await
        .expect("third CreateCursor must unblock after a release")
        .expect("dispatch task must not panic");
    assert!(matches!(resp3, DbResponse::CursorPage { .. }));
    let _guard3 = guard3.expect("bounded budget must stash a guard");

    drop(guard_b);
    drop(_guard3);
    assert_eq!(
        budget.used(),
        0,
        "every guard released; budget must be fully drained"
    );
}

/// A guard acquired for a cursor page is released after the simulated
/// writer-task write-error path — mirrors
/// `byte_budget_exhaustion_tests::release_on_simulated_write_error_recovers_budget`.
#[tokio::test]
async fn cursor_page_guard_released_on_simulated_write_error_recovers_budget() {
    let one_page = measure_one_page_size(50, 50).await;
    let cap = one_page + one_page / 2;
    let budget = ByteBudget::new(Some(cap));
    let handler = build_handler(50, budget.clone(), QueryLimitsCap::UNLIMITED).await;

    let session = alice_session();
    let (resp, guard) = send_with_guard(&handler, &session, create_cursor_req(50)).await;
    assert!(matches!(resp, DbResponse::CursorPage { .. }));
    let guard = guard.expect("bounded budget must stash a guard");
    assert!(budget.used() >= one_page);

    // Simulate the writer task's error path: dropping the guard without any
    // successful write ever having happened must still release the bytes.
    drop(guard);

    assert_eq!(
        budget.used(),
        0,
        "a guard dropped on the write-error path must still release its bytes"
    );

    // Budget must be usable again immediately — no permanent leak.
    let (resp2, guard2) = send_with_guard(&handler, &session, create_cursor_req(50)).await;
    assert!(matches!(resp2, DbResponse::CursorPage { .. }));
    drop(guard2);
}

// ---------------------------------------------------------------------------
// CR-B2 — upfront-reserve-then-shrink parity for the cursor path
// (`enforce_page_budget` via `reserve_page_budget_upfront`).
// ---------------------------------------------------------------------------

/// With an ACTIVE per-page size cap (`max_result_size_bytes` <
/// `usize::MAX`), `FetchNext` reserves upfront using that cap as the
/// pessimistic estimate BEFORE running the pinned-version read for the
/// page — mirrors `byte_budget_upfront_reserve_tests`'s proof for the
/// `Execute` path, exercised through `FetchNext` instead.
///
/// A budget cap sized for exactly ONE page's upfront estimate: a second,
/// concurrent `FetchNext` must block (its execution, not just its write)
/// until the first page's guard is released — proving the reservation
/// happens before the page is built, not after.
#[tokio::test]
async fn fetch_next_upfront_reserve_blocks_second_page_before_it_completes() {
    let one_page = measure_one_page_size(50, 2).await;
    assert!(one_page > 0, "sanity: page must be non-empty");

    // Tight cap close to the actual page size (not a loose multiple) so the
    // upfront estimate and the actual size stay close together — same
    // rationale as `byte_budget_upfront_reserve_tests`'s cap sizing.
    let max_result_size_bytes = one_page + one_page / 10;
    let budget = ByteBudget::new(Some(max_result_size_bytes));
    let query_limits = QueryLimitsCap {
        max_result_size_bytes,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = Arc::new(build_handler(50, budget.clone(), query_limits).await);

    // Open the cursor first (its own first page must fit the cap too, since
    // CreateCursor goes through the identical `enforce_page_budget` gate).
    let session = alice_session();
    let (resp0, guard0) = send_with_guard(&handler, &session, create_cursor_req(2)).await;
    let cursor_id = match resp0 {
        DbResponse::CursorPage {
            cursor_id,
            has_more,
            ..
        } => {
            assert!(has_more, "48 of 50 rows remain after the first page");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    drop(guard0);
    assert_eq!(
        budget.used(),
        0,
        "the first page's guard must be released before the blocking assertions below"
    );

    // First FetchNext: hold its guard WITHOUT dropping it (simulates a page
    // still in flight on the write path).
    let (resp1, guard1) = send_with_guard(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    assert!(matches!(resp1, DbResponse::CursorPage { .. }));
    let guard1 = guard1.expect("bounded budget must stash a guard");
    assert!(
        budget.used() > 0,
        "first FetchNext's (shrunk) reservation must still be held"
    );

    // A second, concurrent FetchNext must now be blocked — its execution
    // cannot proceed because the upfront acquire for its own page-size
    // estimate cannot fit alongside the first (still-held) reservation.
    let handler_clone = Arc::clone(&handler);
    let second = tokio::spawn(async move {
        let session = alice_session();
        send_with_guard(&handler_clone, &session, fetch_next_req(cursor_id, 2)).await
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !second.is_finished(),
        "second FetchNext's EXECUTION must be blocked at the upfront acquire — the R-2 proof \
         applied to the cursor path: without the fix, `enforce_page_budget` only acquires AFTER \
         the page is built, so this task would already have completed regardless of the tiny cap"
    );

    // Releasing the first guard frees enough room for the second's upfront
    // reservation to proceed.
    drop(guard1);

    let (resp2, guard2) = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("second FetchNext must unblock once the first guard is released")
        .expect("dispatch task must not panic");
    assert!(matches!(resp2, DbResponse::CursorPage { .. }));
    let guard2 = guard2.expect("bounded budget must stash a guard");
    drop(guard2);
    assert_eq!(budget.used(), 0);
}

/// After a `FetchNext` completes, `budget.used()` reflects the ACTUAL page
/// size, not the (larger) upfront `max_result_size_bytes` estimate — proves
/// `enforce_page_budget`'s `shrink_to` call ran end-to-end.
#[tokio::test]
async fn fetch_next_shrinks_upfront_reservation_to_actual_page_size() {
    let one_page = measure_one_page_size(50, 2).await;

    // A deliberately generous cap — much bigger than the actual 2-row page
    // — so the upfront estimate and the actual size are clearly
    // distinguishable.
    let max_result_size_bytes = one_page * 20;
    let budget = ByteBudget::new(Some(max_result_size_bytes * 2));
    let query_limits = QueryLimitsCap {
        max_result_size_bytes,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(50, budget.clone(), query_limits).await;

    let session = alice_session();
    let (resp0, guard0) = send_with_guard(&handler, &session, create_cursor_req(2)).await;
    let cursor_id = match resp0 {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };
    drop(guard0);
    assert_eq!(budget.used(), 0);

    let (resp1, guard1) = send_with_guard(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    assert!(matches!(resp1, DbResponse::CursorPage { .. }));
    let guard1 = guard1.expect("bounded budget must stash a guard");

    let used = budget.used();
    assert!(
        used < max_result_size_bytes,
        "budget.used() ({used}) must reflect the SHRUNK actual page size, not the upfront \
         estimate ({max_result_size_bytes})"
    );
    assert!(
        used + 16 >= one_page,
        "budget.used() ({used}) must be roughly the actual page size ({one_page})"
    );

    drop(guard1);
    assert_eq!(budget.used(), 0);
}

// ---------------------------------------------------------------------------
// Per-page byte-size cap (`query_limits.max_result_size_bytes`).
// ---------------------------------------------------------------------------

/// A cursor page whose serialized size exceeds a configured
/// `max_result_size_bytes` is rejected with `cursor_page_too_large` — and no
/// budget is acquired for the rejected attempt.
#[tokio::test]
async fn cursor_page_too_large_is_rejected_and_reserves_no_budget() {
    let one_page = measure_one_page_size(50, 50).await;
    // Cap just under one page's size -> the first CreateCursor's page must
    // be rejected.
    let max_result_size_bytes = one_page - 1;
    let budget = ByteBudget::new(Some(one_page * 10));
    let handler = build_handler(
        50,
        budget.clone(),
        QueryLimitsCap {
            max_result_size_bytes,
            ..QueryLimitsCap::UNLIMITED
        },
    )
    .await;

    let pre_attempt_used = budget.used();
    assert_eq!(pre_attempt_used, 0);

    let session = alice_session();
    let (resp, guard) = send_with_guard(&handler, &session, create_cursor_req(50)).await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "cursor_page_too_large");
            assert!(
                message.contains(&max_result_size_bytes.to_string()),
                "message should mention the configured max: {message}"
            );
        }
        other => panic!("expected cursor_page_too_large, got {other:?}"),
    }
    assert!(
        guard.is_none(),
        "a rejected too-large page must not stash a budget guard"
    );
    assert_eq!(
        budget.used(),
        pre_attempt_used,
        "budget must be untouched by a rejected too-large page"
    );
}

/// Same rejection, exercised through `FetchNext` on an already-open cursor
/// (page 2), and the cursor's bookmark must remain untouched by the
/// rejection — a subsequent smaller-page-size fetch still works from where
/// the cursor actually left off.
#[tokio::test]
async fn fetch_next_page_too_large_leaves_cursor_bookmark_untouched() {
    // Use a small first page (page_size 2) so CreateCursor succeeds, then
    // request a larger page_size on FetchNext that would exceed the cap.
    let small_page = measure_one_page_size(50, 2).await;
    let large_page = measure_one_page_size(50, 50).await;
    assert!(
        large_page > small_page,
        "sanity: a 50-row page must serialize larger than a 2-row page"
    );

    let max_result_size_bytes = small_page + (large_page - small_page) / 2;
    assert!(
        max_result_size_bytes < large_page,
        "cap must sit strictly below the large page's size"
    );
    let budget = ByteBudget::unbounded();
    let handler = build_handler(
        50,
        budget,
        QueryLimitsCap {
            max_result_size_bytes,
            ..QueryLimitsCap::UNLIMITED
        },
    )
    .await;

    let session = alice_session();
    let (resp, _g) = send_with_guard(&handler, &session, create_cursor_req(2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id,
            has_more,
            ..
        } => {
            assert!(has_more);
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // A FetchNext asking for a large page (50 rows) exceeds the cap and is
    // rejected.
    let (resp2, guard2) = send_with_guard(&handler, &session, fetch_next_req(cursor_id, 50)).await;
    match resp2 {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_page_too_large"),
        other => panic!("expected cursor_page_too_large, got {other:?}"),
    }
    assert!(guard2.is_none());

    // The cursor must remain usable: a subsequent FetchNext with a small
    // page_size (within the cap) continues correctly from page 1's
    // bookmark (rows 3/4), not from some corrupted/advanced state.
    let (resp3, _g3) = send_with_guard(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp3 {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2, "page 2 must still have 2 rows");
            assert!(has_more, "48 of 50 remain -> more pages remain");
        }
        other => {
            panic!("cursor must remain usable after a rejected too-large FetchNext, got {other:?}")
        }
    }
}

// ---------------------------------------------------------------------------
// Regression guards: unbounded budget / within-limits pages unaffected.
// ---------------------------------------------------------------------------

/// With an unbounded budget (the default) and an effectively-unlimited size
/// cap, cursor calls behave exactly as before this task — no guard is
/// stashed, no rejection.
#[tokio::test]
async fn unbounded_budget_and_unlimited_cap_is_a_pure_noop() {
    let handler = build_handler(5, ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let (resp, guard) = send_with_guard(&handler, &session, create_cursor_req(2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more);
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
    assert!(
        guard.is_none(),
        "an unbounded budget must never stash a guard"
    );
}

/// A page comfortably within a configured (bounded) `max_result_size_bytes`
/// is accepted normally and still acquires the RI-15 budget.
#[tokio::test]
async fn page_within_size_cap_is_accepted_and_still_acquires_budget() {
    let one_page = measure_one_page_size(5, 5).await;
    let budget = ByteBudget::new(Some(one_page * 10));
    let handler = build_handler(
        5,
        budget.clone(),
        QueryLimitsCap {
            max_result_size_bytes: one_page * 10,
            ..QueryLimitsCap::UNLIMITED
        },
    )
    .await;
    let session = alice_session();

    let (resp, guard) = send_with_guard(&handler, &session, create_cursor_req(5)).await;
    match resp {
        DbResponse::CursorPage { page, .. } => {
            assert_eq!(page.records.len(), 5);
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
    let guard = guard.expect("bounded budget must stash a guard for an accepted page");
    // Tolerance: `one_page` was measured from a SEPARATE execution
    // (`measure_one_page_size`); a page's serialized size can vary by a
    // few bytes call-to-call (e.g. a timing-derived field crossing a
    // msgpack varint width boundary) — same class of jitter fixed for
    // RI-15's `exhaustion_blocks_until_release` test. A 16-byte tolerance
    // is generous against encoding jitter while still catching a real
    // accounting bug (which would be off by far more than a few bytes).
    let used = budget.used();
    assert!(
        used + 16 >= one_page,
        "accepted page must reserve roughly one page's bytes (used={used}, one_page={one_page})"
    );
    drop(guard);
    assert_eq!(budget.used(), 0);
}
