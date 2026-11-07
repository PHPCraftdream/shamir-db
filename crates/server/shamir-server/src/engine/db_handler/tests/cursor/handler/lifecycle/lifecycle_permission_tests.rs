//! Lifecycle, ACL and permission tests for the cursor wire handlers
//! (create/fetch/cancel happy path, snapshot stability, per-session cap,
//! idle eviction, ownership and mid-scroll permission/ACL changes).

use std::sync::Arc;
use std::time::Duration;

use shamir_db::access::{principal64, Actor};

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::chmod;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::ResourceRef;
use shamir_query_types::batch::BatchError;
use shamir_query_types::read::{
    OrderBy, ReadQuery, Select, SelectExpr, SelectExprValue, SelectItem, Temporal,
};
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use crate::db_handler::config::CursorLimitsCap;

use super::super::common::*;

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
/// style, driving `sweep_and_reap` directly rather than sleeping the real
/// production duration).
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
    let reaped = registry.sweep_and_reap(std::time::Instant::now(), Duration::ZERO);
    assert_eq!(reaped, 1, "cursor must be idle-expired at ttl=0");
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
    // calls sweep_and_reap", mirroring
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

/// #1024 (follow-up to F-26 / #819) — a cursor's every internal read
/// rewrites `temporal` to `Temporal::AsOf { at: At::Version(pinned_version) }`
/// internally (`create_cursor`'s doc comment above), routing through
/// `TableManager::read_as_of` in `shamir-engine`'s `read_temporal.rs` —
/// exercising the SAME `SelectProjection::new` evaluation path as a plain
/// temporal read, but through the cursor wire entry point specifically. A
/// computed `SelectItem::Expression` in the cursor's SELECT must now
/// EVALUATE and surface its computed value in the `CursorPage`, not error
/// (proves the old `select_expression_not_supported` rejection is gone from
/// the cursor path too).
#[tokio::test]
async fn create_cursor_evaluates_select_expression() {
    let handler = build_handler_with_rows(3, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items")
        .select(Select {
            items: vec![
                SelectItem::Field {
                    path: vec!["v".to_string()],
                    alias: None,
                },
                SelectItem::Expression {
                    expr: SelectExpr::Add {
                        left: Box::new(SelectExpr::Field {
                            path: vec!["v".to_string()],
                        }),
                        right: Box::new(SelectExpr::Literal {
                            value: SelectExprValue::Int(1),
                        }),
                    },
                    alias: Some("bumped".to_string()),
                },
            ],
            distinct: false,
        })
        .order_by(OrderBy::asc("v"));

    let resp = send(&handler, &session, create_cursor_req(query, 3)).await;
    match resp {
        DbResponse::CursorPage { page, .. } => {
            assert_eq!(page.records.len(), 3);
            for r in &page.records {
                let v = r.get_value_i64("v").expect("v must be present");
                let bumped = r.get_value_i64("bumped").expect("bumped must be present");
                assert_eq!(bumped, v + 1, "bumped must equal v + 1");
            }
        }
        other => panic!(
            "a computed SelectItem::Expression must evaluate through the \
             cursor path, got {other:?}"
        ),
    }
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
    close_batch.chmod(
        "close",
        chmod(resource, 0o700)
            .hmac("direct-db-test")
            .build()
            .expect("build chmod"),
    );
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

/// W-6 (#790, Wave D review): regression guard pinning the EXACT (accepted,
/// documented) behavior change from CR-D5/N-9's `fetch_next` authorize-
/// before-resolve reorder — a db dropped between `CreateCursor` and a later
/// `FetchNext` now surfaces as `access_denied`, NOT `unknown_db`, for a
/// non-admin actor.
///
/// Mechanism (see the comment at `fetch_next`'s N-9/W-6 authorize call
/// site): `ShamirDb::remove_db` only deletes the db's OWN catalogue row
/// (`TABLE_DATABASES`) plus the in-memory `dbs` registry entry — the
/// table's own persisted ACL (`TABLE_TABLES`, untouched) survives. Under
/// the NEW order, `authorize_cursor_read` runs first and denies bob (a
/// non-owner) via that still-real, still-enforced table ACL BEFORE
/// `resolve_repo` ever gets a chance to run and return `unknown_db` for
/// the now-missing db. This is NOT a bug fix — it is a regression guard for
/// the intentionally-accepted new behavior (the review calls it
/// "defensible" / "arguably more correct"); it must never silently flip
/// back to `unknown_db` without that being a deliberate, reviewed change.
#[tokio::test]
async fn fetch_next_after_db_dropped_mid_scroll_surfaces_access_denied_not_unknown_db() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let bob = other_session();

    // Bob needs Execute on every ancestor (db/store/table) to CREATE the
    // cursor at all -- open all three first (same setup
    // `fetch_next_denies_after_permission_revoked_mid_scroll` uses), then
    // narrow the table back down below before the db itself is dropped.
    open_app_main_items(&handler, 0o777).await;

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &bob, create_cursor_req(query, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage once opened, got {other:?}"),
    };

    // Re-close the table to its enforced-default 0o700 (owner-only) AND
    // drop the db's own catalogue/registry entry -- mirrors "db dropped
    // mid-scroll": the table's real ACL (denying bob) survives the drop
    // untouched, only the db's own row/registry entry disappears.
    let alice_owner = Actor::User(principal64([0xAB; 16]));
    let resource = ResourceRef::Table {
        table: ["app".into(), "main".into(), "items".into()],
    };
    let mut close_batch = Batch::new();
    close_batch.chmod(
        "close",
        chmod(resource, 0o700)
            .hmac("direct-db-test")
            .build()
            .expect("build chmod"),
    );
    handler
        .db()
        .execute_as(alice_owner, "app", &close_batch.build())
        .await
        .expect("owner chmod back to 0o700 must succeed");

    let removed = handler.db().remove_db("app").await.unwrap();
    assert!(removed, "sanity: the db must have actually been removed");

    let resp = send(&handler, &bob, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::Error { code, .. } => {
            assert_eq!(
                code, "access_denied",
                "a db dropped mid-scroll must surface as access_denied (the NEW, \
                 accepted CR-D5/N-9 authorize-before-resolve order), not unknown_db \
                 (the OLD, pre-reorder behavior) -- see W-6 (#790)"
            );
        }
        other => {
            panic!("expected access_denied for a db dropped mid-scroll (W-6, #790), got {other:?}")
        }
    }
}
