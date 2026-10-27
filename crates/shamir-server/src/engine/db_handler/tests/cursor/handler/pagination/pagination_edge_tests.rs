//! CR-B4 `has_more` peek-ahead edge tests, mid-scroll `DropTable`
//! error surfacing, and unknown db/repo error-code tests.

use shamir_db::access::{principal64, Actor};
use shamir_query_types::read::{OrderBy, ReadQuery};

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::drop_table;
use shamir_query_builder::doc;
use shamir_query_types::batch::BatchError;
use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::db_handler::config::CursorLimitsCap;
use crate::version::CURRENT_QUERY_LANG_VERSION;

use super::super::common::*;

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

// ---------------------------------------------------------------------------
// CR-C1 (#776, P-6) — `fetch_next` re-resolves db -> repo -> table on EVERY
// page rather than caching a `TableManager` handle on the cursor. This test
// proves the behavior that decision is deliberately preserving: a `DropTable`
// committed while a cursor is open mid-scroll must be observed as a clean
// error on the NEXT FetchNext, not silently ignored (which a cached handle
// would risk, since nothing on a cached `TableManager` re-checks the
// catalog).
// ---------------------------------------------------------------------------

/// A `DropTable` that commits WHILE a cursor is open mid-scroll must cause
/// the next `FetchNext` to fail cleanly (not panic, not silently keep
/// returning rows as if the table still existed) -- proving `fetch_next`'s
/// per-page `resolve_repo`/`get_table` re-resolution actually observes a
/// concurrent schema change rather than serving stale reads against a
/// conceptually-dropped table.
#[tokio::test]
async fn mid_scroll_drop_table_produces_clean_error_not_stale_reads() {
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();
    let owner = Actor::User(principal64([0xAB; 16]));

    // page_size 2 over 5 rows -> multiple pages; cursor stays open after
    // the first page.
    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
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

    // Drop the table out from under the still-open cursor, via a SEPARATE,
    // real DDL batch (not a fixture-level shortcut) -- exercises the actual
    // concurrent-schema-change path.
    let mut drop_batch = Batch::new();
    drop_batch.drop_table(
        "dt",
        drop_table("items")
            .hmac("direct-db-test")
            .build()
            .expect("build drop table"),
    );
    handler
        .db()
        .execute_as(owner, "app", &drop_batch.build())
        .await
        .expect("drop table must succeed");

    // The next FetchNext must fail cleanly -- not panic, not a CursorPage
    // (which would mean stale reads against a dropped table went through).
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
    match resp {
        DbResponse::Error { code, .. } => {
            // Not asserting a specific code here (this is a generic engine
            // "table not found" surfaced via `wrap_engine_err`, not one of
            // the cursor-specific structured codes) -- the point is that it
            // IS an error, not a silently-served page.
            assert_ne!(
                code, "",
                "a concurrent drop_table mid-scroll must produce SOME clean error code"
            );
        }
        other => panic!(
            "expected a clean error after a concurrent drop_table mid-scroll \
             (proving fetch_next's per-page re-resolution observes the schema \
             change instead of serving stale reads), got {other:?}"
        ),
    }
}

// ---------------------------------------------------------------------------
// CR-C1 (#776, B-5) — `resolve_repo`'s db-not-found vs. repo-not-found cases
// must produce DISTINCT wire error codes: `unknown_db` for a nonexistent
// database (unchanged, now explicit instead of accidentally derived from
// `error_code()`'s legacy message-sniffing heuristic) and the NEW
// `unknown_repo` for a database that exists but whose repo does not.
// Previously both cases fell into the SAME heuristic branch
// (`alias.is_empty() && message.contains("not found")`) and were both
// misreported as `unknown_db`, which is misleading when the database
// genuinely exists and only the repo inside it doesn't.
// ---------------------------------------------------------------------------

/// `CreateCursor` against a database that does not exist at all still
/// reports `unknown_db` (regression -- this task must not change the
/// db-not-found wire code, only make it explicit).
#[tokio::test]
async fn create_cursor_against_unknown_db_reports_unknown_db() {
    let handler = build_handler_with_rows(0, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items");
    let req = DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "does_not_exist".to_string(),
        query,
        page_size: 10,
    };
    let resp = send(&handler, &session, req).await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "unknown_db");
            assert!(message.contains("not found"), "got {message:?}");
        }
        other => panic!("expected unknown_db Error, got {other:?}"),
    }
}

/// `CreateCursor` against a database that DOES exist, but whose repo does
/// not, must report the NEW distinct `unknown_repo` code -- not the
/// misleading `unknown_db` a repo-not-found error used to fall back to.
#[tokio::test]
async fn create_cursor_against_unknown_repo_reports_unknown_repo_not_unknown_db() {
    let handler = build_handler_with_rows(0, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // `app` (the database `build_handler_with_rows` created) exists, but
    // its query targets a repo ("does_not_exist_repo") that was never added.
    let mut query = ReadQuery::new("items");
    query.from.repo = "does_not_exist_repo".to_string();

    let resp = send(&handler, &session, create_cursor_req(query, 10)).await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "unknown_repo",
                "a repo-not-found error (db exists, repo doesn't) must NOT be reported as \
                 unknown_db -- got message: {message:?}"
            );
            assert!(message.contains("not found"), "got {message:?}");
        }
        other => panic!("expected unknown_repo Error, got {other:?}"),
    }
}

/// Sanity: `resolve_repo`'s two error paths map through `error_code()` to
/// the expected structured codes directly at the `BatchError` level --
/// belt-and-braces alongside the wire-level assertions above.
#[test]
fn resolve_repo_error_codes_are_distinct_structured_codes() {
    let db_not_found = BatchError::QueryError {
        alias: String::new(),
        message: "Database 'x' not found".to_string(),
        code: Some("unknown_db".to_string()),
    };
    let repo_not_found = BatchError::QueryError {
        alias: String::new(),
        message: "Repository 'y' not found".to_string(),
        code: Some("unknown_repo".to_string()),
    };
    assert_eq!(
        crate::db_handler::handler::error_code(&db_not_found),
        "unknown_db"
    );
    assert_eq!(
        crate::db_handler::handler::error_code(&repo_not_found),
        "unknown_repo"
    );
}
