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
use shamir_query_builder::ddl::chmod;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::ResourceRef;
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

/// Explicit per-call `page_size` override (the pre-CR-B3 always-present
/// shape) — wraps `Some(page_size)` so the many existing call sites below
/// don't need to change.
fn fetch_next_req(cursor_id: CursorId, page_size: u32) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size: Some(page_size),
    }
}

/// CR-B3 (#769): `FetchNext` that OMITS `page_size` — server falls back to
/// the cursor's stored `CreateCursor`-time default.
fn fetch_next_default_req(cursor_id: CursorId) -> DbRequest {
    DbRequest::FetchNext {
        cursor_id,
        page_size: None,
    }
}

/// Open `app.main.items` to `mode` at all three ancestor levels (database,
/// store, table) — `authorize_access`'s ancestor-walk requires `Execute` on
/// EVERY ancestor, not just the target, so a non-owner needs all three
/// chmod'd (mirrors `permission_e2e.rs`'s
/// `permission_open_default_allows_any_user` "chmod db + repo + table to
/// OPEN" sequence).
async fn open_app_main_items(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "db",
        chmod(
            ResourceRef::Database {
                database: "app".into(),
            },
            mode,
        ),
    );
    batch.chmod(
        "store",
        chmod(
            ResourceRef::Store {
                store: ["app".into(), "main".into()],
            },
            mode,
        ),
    );
    batch.chmod(
        "table",
        chmod(
            ResourceRef::Table {
                table: ["app".into(), "main".into(), "items".into()],
            },
            mode,
        ),
    );
    handler
        .db()
        .execute_as(Actor::System, "app", &batch.build())
        .await
        .expect("admin chmod must succeed");
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
            max_cursor_page_size: u32::MAX,
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

    // A DIFFERENT session is unaffected by alice's cap. Bob is not the
    // table's owner (CR-A1 enforces the ACL on the cursor path too, so an
    // unrelated user is denied by the enforced 0o700 default regardless of
    // cap) — open db+store+table so this assertion isolates the CAP
    // behavior from the ACL behavior (covered separately by
    // `create_cursor_denies_non_owner_without_grant`).
    open_app_main_items(&handler, 0o777).await;

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
            max_cursor_page_size: u32::MAX,
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

// ---------------------------------------------------------------------------
// CR-B5 (#771) — reject `with_version = true` at `CreateCursor`.
//
// A cursor's every internal read (both `create_cursor`'s first page and
// every `fetch_next`) rewrites `temporal` to `Temporal::AsOf { at:
// At::Version(pinned_version) }`, and that read path hard-codes
// `versions: None` on the `QueryResult` it returns. Without this rejection,
// `ReadQuery.with_version = true` would silently produce NO per-record
// versions when the query is run through a cursor instead of a plain read —
// a correctness-relevant feature (the FG-2 optimistic-CAS contour) quietly
// stopping to work with no error. `CreateCursor` must reject the
// combination outright instead.
// ---------------------------------------------------------------------------

/// `CreateCursor` with `query.with_version = true` must return the new,
/// distinct error code — NOT a `CursorPage` with silently-missing versions.
#[tokio::test]
async fn create_cursor_rejects_with_version_true() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let mut query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    query.with_version = true;

    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "cursor_with_version_not_supported");
            assert!(
                message.contains("with_version"),
                "message should explain the rejected flag: {message}"
            );
        }
        other => panic!(
            "with_version = true must be rejected with a distinct error, not accepted \
             (which would silently return a CursorPage missing per-record versions), \
             got {other:?}"
        ),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a rejected with_version=true CreateCursor must not register a cursor"
    );
}

/// Sanity: the `BatchError` variant this rejection maps through is a real,
/// distinct enum member — belt-and-braces alongside the wire-level
/// assertion above, mirroring `cursor_temporal_not_supported_is_a_distinct_
/// batch_error_variant`'s pattern.
#[test]
fn cursor_with_version_not_supported_is_a_distinct_batch_error_variant() {
    let e = BatchError::CursorWithVersionNotSupported;
    assert_eq!(
        crate::db_handler::handler::error_code(&e),
        "cursor_with_version_not_supported"
    );
}

/// Regression guard: a PLAIN (non-cursor) read with `with_version = true`
/// still returns real per-record versions — proves this task's cursor-side
/// rejection didn't touch the working, non-cursor batch `Execute`/`Read`
/// path.
#[tokio::test]
async fn plain_read_with_version_true_still_returns_versions_regression() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let owner = Actor::User(principal64([0xAB; 16]));

    let mut batch = Batch::new();
    batch.query(
        "r",
        shamir_query_builder::query::Query::from("items").with_version(),
    );
    let resp = handler
        .db()
        .execute_as(owner, "app", &batch.build())
        .await
        .expect("plain read must succeed");

    let result = resp.results.get("r").expect("alias r present");
    assert_eq!(result.records.len(), 3, "all 3 seeded rows returned");
    assert!(
        result.versions.is_some(),
        "with_version=true on a PLAIN (non-cursor) read must still attach versions \
         (regression guard: this task must not touch the working non-cursor path)"
    );
    assert_eq!(
        result.versions.as_ref().unwrap().len(),
        3,
        "one version per returned record"
    );
}

// ---------------------------------------------------------------------------
// CR-A1 (#760) — ACL/DAC enforcement on the cursor create/fetch path.
//
// `build_handler_with_rows` creates `app.main.items` owned by alice
// (`Actor::User(principal64([0xAB; 16]))`, matching `alice_session()`'s
// `user_id`). New tables default to the enforced 0o700 (owner-rwx-only)
// mode (see `permission_e2e.rs::permission_open_default_allows_any_user`,
// "G.4c" note) — so bob (`other_session()`, a distinct non-owner user id)
// is denied by default with NO chmod needed to set up the negative case.
// ---------------------------------------------------------------------------

/// Bob (non-owner, no grant) attempts `CreateCursor` against alice's table
/// → must be denied with `access_denied`, and no cursor may be registered as
/// a side effect of the attempt.
#[tokio::test]
async fn create_cursor_denies_non_owner_without_grant() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let bob = other_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &bob, create_cursor_req(query, 2)).await;

    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "enforced default (0o700) must deny bob"
            )
        }
        other => panic!("expected access_denied, got {other:?}"),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a denied CreateCursor attempt must not register a cursor"
    );
}

// ---------------------------------------------------------------------------
// CR-A2 (#761) — an already-exhausted first page must NOT be registered:
// no MVCC pin, no per-session cap slot held for a cursor no SDK will ever
// call `FetchNext` against.
// ---------------------------------------------------------------------------

/// Rapid-fire single-page `CreateCursor` calls (each `has_more == false`)
/// must never trip the per-session cursor cap, no matter how many are
/// issued in a row on the same session — none of them are actually
/// registered.
#[tokio::test]
async fn exhausted_first_page_cursors_never_exhaust_the_session_cap() {
    let cap = 2usize;
    let handler = build_handler_with_rows(
        1,
        CursorLimitsCap {
            max_cursors_per_session: cap,
            idle_timeout_secs: u64::MAX,
            max_cursor_page_size: u32::MAX,
        },
    )
    .await;
    let session = alice_session();

    // 1 row in the table, page_size 10 -> every CreateCursor's first page
    // exhausts the whole result (`has_more == false`). Issue more than `cap`
    // of these in a row on the SAME session.
    for i in 0..(cap * 3) {
        let query = ReadQuery::new("items");
        let resp = send(&handler, &session, create_cursor_req(query, 10)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                assert_eq!(page.records.len(), 1);
                assert!(
                    !has_more,
                    "single row / page_size 10 must exhaust on page 1"
                );
            }
            other => panic!("iteration {i}: expected CursorPage, got {other:?}"),
        }
        assert_eq!(
            handler.cursor_registry().len(),
            0,
            "iteration {i}: an exhausted first page must never be registered"
        );
    }
}

/// Empty table: `CreateCursor` returns an empty page with `has_more ==
/// false` and is not registered either (the review's explicit "empty
/// table" case).
#[tokio::test]
async fn create_cursor_over_empty_table_is_not_registered() {
    let handler = build_handler_with_rows(0, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 10)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(page.records.is_empty(), "empty table -> empty page");
            assert!(!has_more, "empty table -> has_more must be false");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "an empty-table cursor must not be registered"
    );
}

/// A `FetchNext` against the id returned by an exhausted (never-registered)
/// `CreateCursor` gets a clean `cursor_not_found`, not a panic.
#[tokio::test]
async fn fetch_next_against_never_registered_exhausted_cursor_is_clean_not_found() {
    let handler = build_handler_with_rows(1, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 10)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id,
            has_more,
            ..
        } => {
            assert!(!has_more);
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 10)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_not_found"),
        other => panic!("expected cursor_not_found, got {other:?}"),
    }
}

/// Regression guard: a query that DOES span multiple pages must still
/// register normally on its first (non-exhausted) page — this fix must not
/// accidentally skip registration when `has_more` is actually `true`.
#[tokio::test]
async fn multi_page_first_page_is_still_registered() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        1,
        "a non-exhausted first page must still be registered"
    );
}

/// Positive control: alice (the owner) creates a cursor on her own table →
/// succeeds normally, proving the new authorize_access calls don't regress
/// the legitimate owner path.
#[tokio::test]
async fn create_cursor_allows_owner() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let alice = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &alice, create_cursor_req(query, 2)).await;

    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more);
        }
        other => panic!("expected CursorPage for the owner, got {other:?}"),
    }
    assert_eq!(handler.cursor_registry().len(), 1);
}

/// A permission revoked BETWEEN `CreateCursor` and a later `FetchNext` must
/// close the read on the very next `FetchNext` — the pinned MVCC snapshot
/// only bounds what data a cursor observes, not whether the actor should
/// still be allowed to observe it at all.
#[tokio::test]
async fn fetch_next_denies_after_permission_revoked_mid_scroll() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let alice_owner = Actor::User(principal64([0xAB; 16]));
    let bob = other_session();

    // Admin opens db+store+table so bob (a non-owner) can create a cursor —
    // `authorize_access`'s ancestor-walk requires Execute on EVERY ancestor,
    // not just the target table.
    open_app_main_items(&handler, 0o777).await;

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &bob, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage once opened, got {other:?}"),
    };

    // Revoke: chmod the table back to owner-only (0o700). Bob no longer
    // qualifies for the Table-level Read check even though db/store are
    // still open.
    let resource = ResourceRef::Table {
        table: ["app".into(), "main".into(), "items".into()],
    };
    let mut close_batch = Batch::new();
    close_batch.chmod("close", chmod(resource, 0o700));
    handler
        .db()
        .execute_as(alice_owner, "app", &close_batch.build())
        .await
        .expect("owner chmod back to 0o700 must succeed");

    let resp = send(&handler, &bob, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "FetchNext must re-check authorization and deny after revocation"
            )
        }
        other => panic!("expected access_denied after mid-scroll revocation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// CR-A3 (#762) — server-side page_size validation: page_size == 0 must
// never reach the has_more == `0 >= 0 -> true` infinite-loop computation,
// and page_size above the configured cap must be rejected outright (not
// silently clamped).
// ---------------------------------------------------------------------------

/// `CreateCursor` with `page_size = 0` must be a clean error, not a
/// `CursorPage` that could loop the client forever — and must not register
/// a cursor as a side effect.
#[tokio::test]
async fn create_cursor_rejects_page_size_zero() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 0)).await;

    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "invalid_page_size");
            assert!(
                message.contains('0'),
                "message should mention the rejected page_size: {message}"
            );
        }
        other => panic!(
            "page_size = 0 must be rejected with a clean error, not a CursorPage \
             (which would loop forever), got {other:?}"
        ),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a page_size = 0 rejection must not register a cursor"
    );
}

/// `FetchNext` with `page_size = 0` against an already-open, still-open
/// cursor must be a clean error — and the cursor itself must remain usable
/// afterward (a bad page_size on one call must not corrupt or close it).
#[tokio::test]
async fn fetch_next_rejects_page_size_zero_and_cursor_survives() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id,
            has_more,
            ..
        } => {
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // A bad FetchNext(page_size=0) is a clean error...
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 0)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "invalid_page_size"),
        other => panic!("expected invalid_page_size, got {other:?}"),
    }

    // ...and the cursor is untouched: a SUBSEQUENT FetchNext with a valid
    // page_size still works normally, continuing from where page 1 left off.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage {
            cursor_id: cid,
            page,
            has_more,
        } => {
            assert_eq!(cid, cursor_id);
            assert_eq!(page.records.len(), 2, "page 2 must still have 2 rows");
            assert!(has_more, "4 of 5 consumed -> one more row remains");
        }
        other => panic!("cursor must remain usable after a bad page_size call, got {other:?}"),
    }
}

/// `CreateCursor` with `page_size` above the configured `max_cursor_page_size`
/// is rejected outright (not silently clamped).
#[tokio::test]
async fn create_cursor_rejects_page_size_above_configured_max() {
    let handler = build_handler_with_rows(
        5,
        CursorLimitsCap {
            max_cursors_per_session: usize::MAX,
            idle_timeout_secs: u64::MAX,
            max_cursor_page_size: 10,
        },
    )
    .await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 11)).await;

    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "invalid_page_size");
            assert!(
                message.contains("10"),
                "message should mention the configured max: {message}"
            );
        }
        other => panic!(
            "page_size above the configured max must be rejected, not clamped, got {other:?}"
        ),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "a page_size-too-large rejection must not register a cursor"
    );
}

/// `FetchNext` with `page_size` above the configured `max_cursor_page_size`
/// is rejected outright, and the cursor remains usable afterward.
#[tokio::test]
async fn fetch_next_rejects_page_size_above_configured_max() {
    let handler = build_handler_with_rows(
        5,
        CursorLimitsCap {
            max_cursors_per_session: usize::MAX,
            idle_timeout_secs: u64::MAX,
            max_cursor_page_size: 10,
        },
    )
    .await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 11)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "invalid_page_size"),
        other => panic!("expected invalid_page_size, got {other:?}"),
    }

    // Cursor survives the rejected call.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more);
        }
        other => panic!(
            "cursor must remain usable after a rejected too-large page_size call, got {other:?}"
        ),
    }
}

// ---------------------------------------------------------------------------
// CR-B3 (#769) — `FetchNext.page_size` is `Option<u32>`: `None` falls back
// to the cursor's stored `CreateCursor`-time default (`Cursor::
// default_page_size`), previously a dead field nothing ever consumed.
// ---------------------------------------------------------------------------

/// A `FetchNext` that OMITS `page_size` (sends `None`) must use the
/// `CreateCursor`-time default page size, not some other value.
#[tokio::test]
async fn fetch_next_omitted_page_size_uses_create_cursor_default() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // CreateCursor-time default is 2 — every FetchNext that omits page_size
    // must keep returning 2-row pages until the data runs out.
    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more);
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Page 2: page_size omitted -> must still return 2 rows (the stored
    // CreateCursor-time default), not some other count.
    let resp = send(&handler, &session, fetch_next_default_req(cursor_id)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(
                page.records.len(),
                2,
                "omitted page_size must fall back to the CreateCursor-time default (2)"
            );
            assert!(has_more, "4 of 5 consumed -> one more row remains");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    // Page 3: page_size omitted again -> final row, has_more -> false.
    let resp = send(&handler, &session, fetch_next_default_req(cursor_id)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 1, "only 1 row left");
            assert!(!has_more, "last page must report has_more == false");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
}

/// Regression: an explicit `Some(n)` on `FetchNext` must still override the
/// `CreateCursor`-time default — the existing per-call-backpressure
/// behavior is unchanged by CR-B3.
#[tokio::test]
async fn fetch_next_explicit_page_size_still_overrides_default() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // CreateCursor-time default is 2, but this FetchNext explicitly asks
    // for 1 row.
    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(
                page.records.len(),
                1,
                "explicit Some(1) must override the CreateCursor-time default (2)"
            );
            assert!(has_more);
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// CR-A4 (#764) — keyset tie-breaker: duplicate ORDER BY values at a page
// boundary must never silently lose (or duplicate) rows.
//
// `build_handler_with_rows` seeds `v` as a strictly-increasing sequence
// (`0..n`), which never exercises a tie. These tests seed `app.main.items`
// with an explicit, caller-chosen `score` sequence (allowing duplicates)
// plus a strictly-unique `seq` field so every row is identifiable even when
// its `score` collides with others — the assertion oracle for "no loss, no
// duplication" is "every `seq` appears in the drained set exactly once".
// ---------------------------------------------------------------------------

/// Build a handler over an in-memory `ShamirDb` with `app.main.items`, owned
/// by alice, seeded with one row per entry in `scores`: `{ seq: i, score:
/// scores[i] }`. `seq` is a strictly-increasing insertion-order tiebreaker
/// field distinct from `score`, letting tests identify every row uniquely
/// even when many rows share the same `score`.
async fn build_handler_with_scores(
    scores: &[i64],
    cursor_limits: CursorLimitsCap,
) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    if !scores.is_empty() {
        let mut b = Batch::new();
        for (i, score) in scores.iter().enumerate() {
            b.insert(
                format!("i{i}"),
                insert("items").row(doc! { "seq" => i as i64, "score" => *score }),
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

/// Drain a cursor (already created, first page already consumed by the
/// caller) via repeated `FetchNext(page_size)` calls until `has_more ==
/// false`, collecting every page's rows' `seq` values (via
/// `get_value_i64("seq")`) into the given accumulator.
async fn drain_seqs(
    handler: &ShamirDbHandler,
    session: &Session,
    cursor_id: CursorId,
    page_size: u32,
    out: &mut Vec<i64>,
) {
    loop {
        let resp = send(handler, session, fetch_next_req(cursor_id, page_size)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    out.push(
                        r.get_value_i64("seq")
                            .expect("every row must carry a seq field"),
                    );
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage while draining, got {other:?}"),
        }
    }
}

/// The review's exact scenario: 4 rows all tied at `score: 10`,
/// `page_size: 2` — draining the whole cursor must return each of the 4
/// rows exactly once (no loss from the old exclusive `Gt` boundary, no
/// duplication from the new inclusive `Gte` + skip-past-tie-run scheme).
#[tokio::test]
async fn keyset_tie_run_exactly_one_page_boundary_no_loss() {
    let handler = build_handler_with_scores(&[10, 10, 10, 10], CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "4 rows / page_size 2 -> more pages remain");
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3],
        "every one of the 4 tied rows must appear exactly once across the whole cursor lifetime"
    );
}

/// Larger randomized-duplicates case: 20 rows, ORDER BY column has only 4
/// distinct values (heavy duplication), several `page_size`s — the drained
/// set of `seq`s must equal the full `0..20` set exactly (a `HashSet`
/// length check catches both loss AND duplication in one assertion).
#[tokio::test]
async fn keyset_heavy_duplication_no_loss_no_duplication_across_page_sizes() {
    use shamir_collections::TFxSet;

    // 20 rows, 4 distinct score buckets (0, 1, 2, 3), heavily interleaved so
    // ties are NOT contiguous by insertion order alone once ORDER BY sorts
    // them into buckets.
    let scores: Vec<i64> = (0..20).map(|i| i % 4).collect();

    for &page_size in &[1u32, 3, 5, 7, 20] {
        let handler = build_handler_with_scores(&scores, CursorLimitsCap::UNLIMITED).await;
        let session = alice_session();

        let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
        let resp = send(&handler, &session, create_cursor_req(query, page_size)).await;
        let (cursor_id, mut seqs, has_more) = match resp {
            DbResponse::CursorPage {
                cursor_id,
                page,
                has_more,
            } => {
                let seqs: Vec<i64> = page
                    .records
                    .iter()
                    .map(|r| r.get_value_i64("seq").expect("seq present"))
                    .collect();
                (cursor_id, seqs, has_more)
            }
            other => panic!("page_size {page_size}: expected CursorPage, got {other:?}"),
        };

        if has_more {
            drain_seqs(&handler, &session, cursor_id, page_size, &mut seqs).await;
        }

        let set: TFxSet<i64> = seqs.iter().copied().collect();
        assert_eq!(
            set.len(),
            20,
            "page_size {page_size}: every one of the 20 rows must appear exactly once \
             (set.len() != seqs.len() would mean a duplicate; set.len() < 20 would mean a loss) \
             -- got {} rows, {} unique",
            seqs.len(),
            set.len()
        );
        assert_eq!(
            seqs.len(),
            20,
            "page_size {page_size}: total rows returned across the whole cursor lifetime must be exactly 20"
        );
    }
}

/// Boundary run larger than one page: a tie run of 10 identical `score`
/// values with `page_size: 2` — proves the bounded-retry-with-growing-limit
/// logic actually clears a tie run that spans MANY internal fetch attempts
/// (a single `page_size`-sized fetch can only see 2 of the 10 tied rows).
#[tokio::test]
async fn keyset_tie_run_larger_than_one_page_uses_bounded_retry() {
    // 10 rows tied at score=5, then 2 more distinct trailing rows so the
    // cursor has something to report after the tie run clears.
    let mut scores = vec![5i64; 10];
    scores.push(6);
    scores.push(7);

    let handler = build_handler_with_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more);
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        (0..12).collect::<Vec<i64>>(),
        "the 10-row tie run plus 2 trailing rows must all surface exactly once, \
         proving the growing-internal-limit retry clears a tie run bigger than one page"
    );
}

/// Pagination mode pinned at creation: a cursor's `PaginationMode` is
/// decided once from the ORIGINAL query and never re-derived per
/// `FetchNext`. This is a behavioral proxy (internal state isn't exposed
/// through the wire protocol): a cursor whose ORDER BY column is absent
/// from an EARLIER page's projection (so a naive per-call re-derivation
/// might have tried to fall back to a row-count bookmark for that call)
/// must still page correctly end-to-end when the field IS present on every
/// page (the common, correct case) -- proving the mode decision made at
/// creation is stable across the whole scroll, not re-evaluated call by
/// call in a way that could flip-flop.
#[tokio::test]
async fn keyset_pagination_mode_pinned_at_creation_stays_stable() {
    let handler = build_handler_with_scores(&[1, 1, 2, 2, 3, 3], CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more);
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Multiple FetchNext calls in a row, all against the SAME cursor, all
    // still Keyset-mode (never flips to Offset mid-scroll even though
    // several consecutive pages land exactly on a tie boundary).
    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4, 5],
        "all 6 rows (3 tie-pairs) must surface exactly once under a pinned Keyset mode"
    );
}

/// Regression guard: the existing no-duplicates multi-page test
/// (`create_fetch_cancel_happy_path_paginates_all_rows`, unchanged above)
/// stays green under the CR-A4 inclusive-boundary rewrite -- this test adds
/// an explicit non-tied, strictly-increasing-score regression check using
/// the SAME `build_handler_with_scores` helper this file's CR-A4 tests use,
/// so the tie-breaker change is proven not to alter behavior for the common
/// (non-tied) case.
#[tokio::test]
async fn keyset_no_ties_regression_every_row_returned_once_in_order() {
    let scores: Vec<i64> = (0..7).collect(); // 0,1,2,3,4,5,6 -- all distinct
    let handler = build_handler_with_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 3)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more);
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    drain_seqs(&handler, &session, cursor_id, 3, &mut seqs).await;

    // No ties -> insertion order == score order == seq order; must come
    // back in strict ascending order with no loss or duplication.
    assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6]);
}

// ---------------------------------------------------------------------------
// CR-B1 (#767) — cursor snapshot stability vs. concurrent DELETE.
//
// `CreateCursor`/`FetchNext` read every page via `Temporal::AsOf { at:
// pinned_version }` (see `create_cursor`'s `guard.version()` pin above). The
// bug: `read_as_of`'s enumeration source (`TableManager::list_stream` ->
// `MvccStore::current_stream`) suppresses any key whose CURRENT winner is a
// tombstone -- so a row alive at the cursor's pinned version, but deleted by
// a separate concurrent write mid-scroll, silently vanishes from every
// subsequent page instead of surfacing its pinned pre-delete value.
// ---------------------------------------------------------------------------

/// A row that has NOT yet been fetched is deleted (via a separate batch)
/// while the cursor is mid-scroll. Draining the rest of the cursor must
/// still surface that row exactly once (its pinned pre-delete value) --
/// before the fix, `read_as_of`'s enumeration drops it entirely because its
/// current winner is now a tombstone.
#[tokio::test]
async fn cursor_still_returns_a_row_deleted_mid_scroll() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // Small page_size so multiple FetchNext calls are needed to drain all 5
    // rows (v = 0..5), ordered so the delete target (v=2) is not yet fetched
    // after the first page.
    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id, page, ..
        } => {
            let vs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("v").unwrap())
                .collect();
            assert_eq!(vs, vec![0, 1], "first page covers v=0,1 only");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Concurrent DELETE of a row NOT yet fetched (v=2, "k002") via a
    // separate, real batch call -- the cursor's pinned snapshot predates
    // this delete.
    let owner = Actor::User(principal64([0xAB; 16]));
    let mut del = Batch::new();
    del.delete(
        "d",
        shamir_query_builder::write::delete("items")
            .where_(shamir_query_builder::filter::eq("id", "k002")),
    );
    handler
        .db()
        .execute_as(owner, "app", &del.build())
        .await
        .expect("concurrent delete must commit");

    // Sanity: a fresh, non-cursor read no longer sees the deleted row.
    let mut fresh = Batch::new();
    fresh.query("r", shamir_query_builder::query::Query::from("items"));
    let fresh_resp = handler
        .db()
        .execute_as(Actor::System, "app", &fresh.build())
        .await
        .expect("fresh read");
    let fresh_result = fresh_resp.results.get("r").expect("alias r present");
    assert_eq!(
        fresh_result.records.len(),
        4,
        "sanity: the concurrent delete is visible to a fresh, non-cursor read"
    );

    // Drain the rest of the cursor. Every row alive at the cursor's pinned
    // version (v = 0..5, INCLUDING the since-deleted v=2) must appear
    // exactly once across all pages.
    let mut total_seen = 2usize; // first page already consumed above
    let mut seen_vs: Vec<i64> = vec![0, 1];
    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                total_seen += page.records.len();
                for r in &page.records {
                    seen_vs.push(r.get_value_i64("v").expect("v present"));
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
    }

    seen_vs.sort_unstable();
    assert_eq!(
        total_seen, 5,
        "cursor must still see all 5 rows pinned at CreateCursor time, \
         including the one deleted mid-scroll"
    );
    assert_eq!(
        seen_vs,
        vec![0, 1, 2, 3, 4],
        "every row alive at the pinned snapshot (v=0..5) must appear exactly \
         once, including v=2 which was deleted AFTER the cursor was created \
         but BEFORE it was fetched"
    );
}

/// Companion regression guard: an UPDATE (not a delete) to a not-yet-fetched
/// row mid-scroll must still show the cursor's PINNED-version value, not the
/// post-update one -- proving this fix (which widens `read_as_of`'s
/// enumeration to include tombstoned keys) does not change the update case,
/// which already worked (the row's current winner is non-empty either way,
/// so it was never excluded from enumeration).
#[tokio::test]
async fn cursor_keeps_pinned_value_for_a_row_updated_mid_scroll() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id, page, ..
        } => {
            let vs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("v").unwrap())
                .collect();
            assert_eq!(vs, vec![0, 1], "first page covers v=0,1 only");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Concurrent UPDATE of a row NOT yet fetched (v=2, "k002") -> v=9999,
    // via a separate, real batch call.
    let owner = Actor::User(principal64([0xAB; 16]));
    let mut upd = Batch::new();
    upd.update(
        "u",
        shamir_query_builder::write::update("items")
            .where_(shamir_query_builder::filter::eq("id", "k002"))
            .set(shamir_query_builder::doc! { "v" => 9999_i64 }),
    );
    handler
        .db()
        .execute_as(owner, "app", &upd.build())
        .await
        .expect("concurrent update must commit");

    // Sanity: a fresh, non-cursor read sees the updated value.
    let mut fresh = Batch::new();
    fresh.query("r", shamir_query_builder::query::Query::from("items"));
    let fresh_resp = handler
        .db()
        .execute_as(Actor::System, "app", &fresh.build())
        .await
        .expect("fresh read");
    let fresh_result = fresh_resp.results.get("r").expect("alias r present");
    let fresh_vs: Vec<i64> = fresh_result
        .records
        .iter()
        .map(|r| r.get_value_i64("v").expect("v present"))
        .collect();
    assert!(
        fresh_vs.contains(&9999),
        "sanity: the concurrent update is visible to a fresh, non-cursor read"
    );

    // Drain the rest of the cursor -- the pinned pre-update value (v=2) must
    // still surface, never the post-update value (v=9999).
    let mut seen_vs: Vec<i64> = vec![0, 1];
    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    seen_vs.push(r.get_value_i64("v").expect("v present"));
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
    }

    seen_vs.sort_unstable();
    assert_eq!(
        seen_vs,
        vec![0, 1, 2, 3, 4],
        "cursor must keep returning the PINNED-version value (v=2), not the \
         post-update value (v=9999), across the rest of its pages"
    );
    assert!(
        !seen_vs.contains(&9999),
        "the post-update value must never appear in any page of this cursor"
    );
}

// ---------------------------------------------------------------------------
// CR-B4 (#770) — `has_more` peek-ahead: an exact-multiple-of-page_size result
// set must report `has_more: false` on the TRUE last page, with no spurious
// extra round-trip, at BOTH buggy call sites (`create_cursor`'s first page,
// `fetch_next`'s offset-bookmark branch). `fetch_keyset_page`'s own
// peek-ahead (CR-A4) is untouched and out of scope for these tests.
// ---------------------------------------------------------------------------

/// `CreateCursor`'s first page: a result set of EXACTLY `page_size` rows
/// must report `has_more: false` immediately, and the cursor must not be
/// registered (mirrors the CR-A2 "exhausted first page is not registered"
/// pattern) -- reusing the SAME machinery proves the peek-ahead fetch (which
/// internally asks for `page_size + 1`) does not accidentally see a
/// nonexistent extra row.
#[tokio::test]
async fn create_cursor_exact_multiple_result_has_more_false_on_first_page() {
    let handler = build_handler_with_rows(2, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;

    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2, "all 2 rows must come back");
            assert!(
                !has_more,
                "2 rows / page_size 2 is an EXACT multiple -- has_more must be false \
                 on the true last page, not a stale 'fetched >= page_size' true"
            );
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "an exact-multiple first page that exhausts the result must not be registered"
    );
}

/// `FetchNext`'s OFFSET-bookmark branch (no simple single-column ORDER BY,
/// so `pagination_mode_for_query` pins `PaginationMode::Offset` for the
/// cursor's whole lifetime): a total row count that's an exact multiple of
/// `page_size` across multiple pages -- the TRUE last page must report
/// `has_more: false` with NO subsequent empty-page round-trip. Assert by
/// counting exactly how many `FetchNext` calls were needed.
#[tokio::test]
async fn fetch_next_offset_path_exact_multiple_result_no_spurious_empty_page() {
    // No `order_by` at all -> `pagination_mode_for_query` falls back to
    // `PaginationMode::Offset` (no simple single-column ORDER BY to keyset
    // seek on).
    let handler = build_handler_with_rows(6, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 2, "first page must have page_size rows");
            assert!(has_more, "6 rows / page_size 2 -> more pages remain");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // 6 rows total, page_size 2, first page already consumed 2 -> exactly 2
    // more FetchNext calls are needed (pages 2 and 3), and the SECOND of
    // those must be the true last page (has_more: false) with no further
    // round-trip required.
    let mut fetch_next_calls = 0u32;
    let mut total_rows = 2usize;
    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
        fetch_next_calls += 1;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                total_rows += page.records.len();
                if !has_more {
                    assert!(
                        !page.records.is_empty(),
                        "the true last page must carry real rows, not be the spurious \
                         empty page the old heuristic would have produced"
                    );
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
        assert!(
            fetch_next_calls <= 3,
            "must not need more than the expected 2 FetchNext calls -- a 3rd would be \
             the spurious empty round-trip this fix eliminates"
        );
    }

    assert_eq!(
        fetch_next_calls, 2,
        "exactly 2 FetchNext calls must drain the remaining 4 rows (2 pages of 2) -- \
         a 3rd (spurious empty) call would mean has_more lied on the true last page"
    );
    assert_eq!(total_rows, 6, "all 6 rows must be seen exactly once");

    // The cursor is not registered anymore -- FetchNext against it now is a
    // clean not-found, proving CR-A2's "not registered on !has_more" cleanup
    // fired at the RIGHT point (the true last page), not one page late.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::Error { code, .. } => assert_eq!(code, "cursor_not_found"),
        other => panic!("expected cursor_not_found after true exhaustion, got {other:?}"),
    }
}

/// Non-multiple results unchanged: an existing partial-final-page scenario
/// (5 rows, page_size 2 -> pages of 2, 2, 1) must still behave exactly as
/// before -- proving the peek-ahead fix doesn't change behavior when there
/// genuinely IS a partial final page. Uses the offset (no-ORDER-BY) path,
/// the branch actually touched by this task.
#[tokio::test]
async fn fetch_next_offset_path_non_multiple_result_unchanged() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
            cursor_id
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 2);
            assert!(has_more, "4 of 5 consumed -> one more row remains");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(
                page.records.len(),
                1,
                "genuine partial final page: only 1 row left"
            );
            assert!(!has_more, "last page must report has_more == false");
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }
}

/// Bookmark correctness across the trimmed peek row, `create_cursor` ->
/// first `FetchNext` transition (offset/no-ORDER-BY path): the row peeked
/// and trimmed off the first page must reappear as the FIRST row of the
/// NEXT page, exactly once (not skipped, not duplicated).
#[tokio::test]
async fn create_cursor_then_fetch_next_offset_path_peek_row_not_skipped_or_duplicated() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // No ORDER BY -> offset/no-keyset path; `build_handler_with_rows` seeds
    // rows in strictly increasing `v` order and the pinned-snapshot full
    // scan enumerates deterministically, so the row-count offset bookmark
    // lines up with `v`'s insertion order exactly like the existing
    // multi-page regression test already relies on.
    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "3 rows / page_size 2 -> more pages remain");
            let vs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("v").expect("v present"))
                .collect();
            (cursor_id, vs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(page.records.len(), 1, "only the trimmed-peek row remains");
            assert!(!has_more);
            for r in &page.records {
                seen_vs.push(r.get_value_i64("v").expect("v present"));
            }
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    assert_eq!(
        seen_vs,
        vec![0, 1, 2],
        "the row peeked-and-trimmed off page 1 must reappear exactly once, as the \
         first (and only) row of page 2 -- not skipped, not duplicated"
    );
}

/// Bookmark correctness across the trimmed peek row, multi-page offset-mode
/// drain (>= 2 FetchNext calls, each of which peeks and trims): every row
/// across the whole cursor lifetime must appear exactly once, in order.
#[tokio::test]
async fn fetch_next_offset_path_multi_page_drain_peek_rows_not_skipped_or_duplicated() {
    let handler = build_handler_with_rows(9, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more);
            let vs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("v").expect("v present"))
                .collect();
            (cursor_id, vs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    seen_vs.push(r.get_value_i64("v").expect("v present"));
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
    }

    assert_eq!(
        seen_vs,
        (0..9).collect::<Vec<i64>>(),
        "every one of the 9 rows must appear exactly once, in order, across a \
         multi-page offset-mode drain where multiple pages each peek and trim \
         a row (9 rows / page_size 2 -> pages of 2,2,2,2,1)"
    );
}

/// Regression guard: the existing CR-A4 tie-breaker tests, the happy-path
/// multi-page drain, and the CR-A2 terminal-page test all stay green under
/// this fix (they already run as part of this file's suite; this test just
/// documents the expectation explicitly for the keyset path, which this
/// task must NOT touch). `fetch_keyset_page`'s own peek-ahead is untouched --
/// this is a direct regression check that a keyset-mode exact-multiple
/// result (already correctly handled pre-CR-B4 by CR-A4's internal
/// limit-plus-one design) is unaffected by this task's offset/first-page
/// changes.
#[tokio::test]
async fn keyset_path_exact_multiple_result_still_correct_untouched_by_this_task() {
    // 4 distinct (non-tied) scores, page_size 2 -> exact multiple, keyset
    // mode (single-column ORDER BY).
    let scores: Vec<i64> = (0..4).collect();
    let handler = build_handler_with_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "4 rows / page_size 2 -> more pages remain");
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    let mut fetch_next_calls = 0u32;
    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
        fetch_next_calls += 1;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    seqs.push(r.get_value_i64("seq").expect("seq present"));
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
        assert!(fetch_next_calls <= 2, "keyset path already peeks correctly (CR-A4) -- must not need a spurious extra round-trip");
    }

    seqs.sort_unstable();
    assert_eq!(seqs, vec![0, 1, 2, 3]);
    assert_eq!(
        fetch_next_calls, 1,
        "keyset path (already correct pre-CR-B4) needs exactly 1 FetchNext to drain \
         the remaining exact-multiple rows"
    );
}
