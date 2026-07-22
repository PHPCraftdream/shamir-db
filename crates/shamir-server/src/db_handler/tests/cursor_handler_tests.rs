//! FG-5b — behavioral tests for `ShamirDbHandler::{create_cursor, fetch_next,
//! cancel_cursor}`, exercised through the wire `RequestHandler::handle`
//! entry point (mirrors `node_mode_tests.rs`'s harness style: a real
//! in-memory `ShamirDb` with an owned table, wire msgpack round-trip).

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
use shamir_query_types::batch::BatchError;
use shamir_query_types::read::{OrderBy, ReadQuery, Temporal};
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use crate::db_handler::config::CursorLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const ALICE_SID: [u8; 32] = [0xAA; 32];
const BOB_SID: [u8; 32] = [0xBB; 32];

/// `Session::new` does not take `session_id` as a constructor argument (it
/// always zero-inits that field; the 6th arg is `channel_binding_at_auth`,
/// a DIFFERENT field) — real sessions get theirs assigned post-construction
/// by the resume/auth path (see `Session::new_authenticated`/`resume` in
/// `shamir-connect`). Tests that need two DISTINCT sessions (for the
/// cross-session ownership / per-session-cap tests below) must set
/// `session_id` explicitly on the public field, or every fixture session
/// collapses to the same all-zero id.
fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

fn other_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "bob".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = BOB_SID;
    s
}

/// Build a handler over an in-memory `ShamirDb` with `app.main.items`,
/// owned by alice (mirrors `node_mode_tests::build_handler`), with
/// `n` rows pre-inserted (`{ id: "kNN", v: NN }`).
async fn build_handler_with_rows(n: usize, cursor_limits: CursorLimitsCap) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    if n > 0 {
        let mut b = Batch::new();
        for i in 0..n {
            b.insert(
                format!("i{i}"),
                insert("items").row(doc! { "id" => format!("k{i:03}"), "v" => i as i64 }),
            );
        }
        let batch = b.build();
        shamir
            .execute_as(owner, "app", &batch)
            .await
            .expect("seed rows");
    }

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = handler
        .handle(session, &bytes, &conn)
        .await
        .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

fn create_cursor_req(query: ReadQuery, page_size: u32) -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query,
        page_size,
    }
}

fn fetch_next_req(cursor_id: CursorId, page_size: u32) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size,
    }
}

// ---------------------------------------------------------------------------
// Happy path: CreateCursor -> FetchNext (repeatable, multiple pages) ->
// CancelCursor, has_more transitions true -> false, post-exhaustion fetch
// is a clean error (not a panic).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_fetch_cancel_happy_path_paginates_all_rows() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;

    let (cursor_id, first_page_has_more) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 2, "first page must have page_size rows");
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
            (cursor_id, has_more)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert!(first_page_has_more);

    // Page 2: 2 more rows, still has_more.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage {
            cursor_id: cid,
            page,
            has_more,
        } => {
            assert_eq!(cid, cursor_id);
            assert_eq!(page.records.len(), 2);
            assert!(has_more, "4 of 5 consumed -> one more row remains");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    // Page 3: final row, has_more transitions to false.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 1, "only 1 row left");
            assert!(!has_more, "last page must report has_more == false");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    // A FetchNext after has_more == false is a clean error, not a panic.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(code, "cursor_not_found");
        }
        other => panic!("expected a clean cursor-not-found error, got {other:?}"),
    }

    // CancelCursor (already auto-closed) is still idempotent-ok.
    let resp = send(&handler, &session, DbRequest::CancelCursor { cursor_id }).await;
    assert!(matches!(resp, DbResponse::CursorClosed { .. }));
}

/// Explicit cancel mid-scroll releases the cursor; a further fetch is a
/// clean not-found error.
#[tokio::test]
async fn cancel_cursor_mid_scroll_then_fetch_is_clean_error() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let resp = send(&handler, &session, DbRequest::CancelCursor { cursor_id }).await;
    assert!(matches!(resp, DbResponse::CursorClosed { cursor_id: cid } if cid == cursor_id));

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_not_found"),
        other => panic!("expected cursor_not_found, got {other:?}"),
    }
}

/// Canceling a cursor id that was never issued is NOT an error (CURSORS.md
/// idempotent-close contract).
#[tokio::test]
async fn cancel_unknown_cursor_is_not_an_error() {
    let handler = build_handler_with_rows(0, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let resp = send(
        &handler,
        &session,
        DbRequest::CancelCursor {
            cursor_id: CursorId(999_999),
        },
    )
    .await;
    assert!(matches!(resp, DbResponse::CursorClosed { .. }));
}

// ---------------------------------------------------------------------------
// Snapshot stability: a write committed AFTER cursor creation, via a
// SEPARATE regular batch call, must NOT be observed by any subsequent page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cursor_does_not_observe_a_write_committed_after_creation() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // Open the cursor over the 3 existing rows, one row per page so the
    // scroll takes multiple FetchNext round-trips (giving the concurrent
    // write plenty of opportunity to be observed if snapshot isolation
    // were broken).
    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id, page, ..
        } => {
            assert_eq!(page.records.len(), 1);
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // A SEPARATE, REAL concurrent write: commit a brand new row through the
    // ordinary (non-cursor) batch path while the cursor is still open and
    // mid-scroll. This exercises the actual concurrent-write path, not a
    // pre-seeded fixture.
    let owner = Actor::User(principal64([0xAB; 16]));
    let mut b = Batch::new();
    b.insert(
        "new_row",
        insert("items").row(doc! { "id" => "k999", "v" => 999_i64 }),
    );
    let write_batch = b.build();
    handler
        .db()
        .execute_as(owner, "app", &write_batch)
        .await
        .expect("concurrent write must commit");

    // Sanity: the write really did land — a fresh, non-cursor read sees 4
    // rows now.
    let mut fresh = Batch::new();
    fresh.query("r", shamir_query_builder::query::Query::from("items"));
    let fresh_batch = fresh.build();
    let fresh_resp = handler
        .db()
        .execute_as(Actor::System, "app", &fresh_batch)
        .await
        .expect("fresh read");
    let fresh_result = fresh_resp.results.get("r").expect("alias r present");
    assert_eq!(
        fresh_result.records.len(),
        4,
        "sanity: the concurrent write is visible to a fresh, non-cursor read"
    );

    // Drain the cursor across all remaining pages; the total records seen
    // across the WHOLE cursor lifetime must be exactly the 3 rows that
    // existed at CreateCursor time — the concurrently-committed 4th row
    // (v=999) must never appear.
    let mut total_seen = 1usize; // first page already consumed above
    let mut seen_v999 = false;
    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                total_seen += page.records.len();
                for r in &page.records {
                    if r.get_value_i64("v") == Some(999) {
                        seen_v999 = true;
                    }
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
    }

    assert_eq!(
        total_seen, 3,
        "cursor must see exactly the 3 rows pinned at CreateCursor time"
    );
    assert!(
        !seen_v999,
        "the row committed AFTER CreateCursor must never appear in any page \
         (proves the pinned snapshot isolates the cursor from concurrent writes)"
    );
}

// ---------------------------------------------------------------------------
// Per-session cap rejection.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_cursor_rejects_past_per_session_cap() {
    let cap = 2usize;
    let handler = build_handler_with_rows(
        5,
        CursorLimitsCap {
            max_cursors_per_session: cap,
            idle_timeout_secs: u64::MAX,
        },
    )
    .await;
    let session = alice_session();

    for _ in 0..cap {
        let query = ReadQuery::new("items");
        let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
        assert!(matches!(resp, DbResponse::CursorPage { .. }));
    }

    // The (cap+1)-th CreateCursor on the SAME session is rejected.
    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_limit_exceeded"),
        other => panic!("expected cursor_limit_exceeded, got {other:?}"),
    }

    // A DIFFERENT session is unaffected by alice's cap.
    let bob = other_session();
    let query = ReadQuery::new("items");
    let resp = send(&handler, &bob, create_cursor_req(query, 1)).await;
    assert!(
        matches!(resp, DbResponse::CursorPage { .. }),
        "another session's cursor cap is independent"
    );
}

// ---------------------------------------------------------------------------
// Idle-timeout eviction.
// ---------------------------------------------------------------------------

/// Create a cursor, don't touch it, advance the reaper's idle-ttl clock via
/// a manual sweep call (no real sleeping — mirrors
/// `tx_registry_tests`'s `reaper_contract_past_deadline_tx_is_removed`
/// style, driving `expired_ids`/`remove_for_idle_reap` directly rather than
/// sleeping the real production duration).
#[tokio::test]
async fn idle_timeout_eviction_then_fetch_returns_expired() {
    let handler = build_handler_with_rows(
        3,
        CursorLimitsCap {
            max_cursors_per_session: 16,
            idle_timeout_secs: 60,
        },
    )
    .await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Drive the reaper's sweep contract directly with a ZERO idle-ttl —
    // the same "shrink the timeout under test" pattern as
    // `commit.rs::TEST_MAX_TX_LIFETIME_OVERRIDE`, applied here via a
    // directly-callable sweep (no real 60s sleep, no background task
    // needed for this assertion).
    let registry = handler.cursor_registry();
    let expired = registry.expired_ids(std::time::Instant::now(), Duration::ZERO);
    assert_eq!(
        expired,
        vec![cursor_id.0],
        "cursor must be idle-expired at ttl=0"
    );
    for id in expired {
        registry.remove_for_idle_reap(id);
    }
    assert!(registry.is_empty());

    // A subsequent FetchNext against the evicted id reports the expired
    // (not merely not-found) error.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_expired"),
        other => panic!("expected cursor_expired, got {other:?}"),
    }
}

/// The background reaper task itself (spawned via
/// `crate::cursor_registry::spawn_reaper_task`) reaps an idle cursor on its
/// own schedule under paused virtual time.
#[tokio::test(start_paused = true)]
async fn background_reaper_evicts_idle_cursor() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let registry = handler.cursor_registry();
    let shutdown = tokio_util::sync::CancellationToken::new();
    // Zero idle-ttl: `Cursor::is_expired` compares `std::time::Instant`
    // deltas, which `tokio::time::advance` does NOT move (only tokio's own
    // timer/interval clock is virtual under `start_paused = true`) — so a
    // non-zero idle_ttl here would never actually elapse in real time no
    // matter how far the virtual clock advances. Zero idle-ttl isolates
    // the assertion to "the reaper's sweep loop fires on schedule and
    // calls remove_for_idle_reap", mirroring
    // `cursor_registry_tests::reaper_task_reaps_idle_cursor`'s fix and
    // `tx_registry_tests::reaper_task_reaps_past_deadline_tx`'s analogous
    // zero-deadline trick.
    let reaper = crate::cursor_registry::spawn_reaper_task(
        Arc::clone(&registry),
        Duration::ZERO,
        Duration::from_millis(50),
        shutdown.clone(),
    );

    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(50)).await;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;

    assert!(
        registry.is_empty(),
        "background reaper drained the idle cursor"
    );
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_expired"),
        other => panic!("expected cursor_expired, got {other:?}"),
    }

    shutdown.cancel();
    let _ = reaper.handle.await;
}

// ---------------------------------------------------------------------------
// Rejected-temporal scope cut.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_cursor_rejects_asof_temporal_not_silently_downgraded() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let mut query = ReadQuery::new("items");
    query.temporal = Temporal::AsOf {
        at: shamir_query_types::read::At::Version(1),
    };

    let resp = send(&handler, &session, create_cursor_req(query, 1)).await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "cursor_temporal_not_supported");
            assert!(
                message.contains("Latest"),
                "message should explain the scope cut: {message}"
            );
        }
        other => panic!(
            "AsOf temporal must be rejected with a distinct error, not silently \
             downgraded to Latest or accepted, got {other:?}"
        ),
    }

    // History is rejected the same way.
    let mut query2 = ReadQuery::new("items");
    query2.temporal = Temporal::History {
        from: None,
        to: None,
        limit: None,
        order: Default::default(),
    };
    let resp2 = send(&handler, &session, create_cursor_req(query2, 1)).await;
    match resp2 {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_temporal_not_supported"),
        other => panic!("History temporal must be rejected, got {other:?}"),
    }
}

/// Sanity: the `BatchError` variant this rejection maps through is a real,
/// distinct enum member (not a generic validation error string-matched by
/// accident) — belt-and-braces alongside the wire-level assertion above.
#[test]
fn cursor_temporal_not_supported_is_a_distinct_batch_error_variant() {
    let e = BatchError::CursorTemporalNotSupported;
    assert_eq!(
        crate::db_handler::handler::error_code(&e),
        "cursor_temporal_not_supported"
    );
}
