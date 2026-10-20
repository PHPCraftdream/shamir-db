//! Offset-bookmark pagination tests (CR-C2) and pinned-snapshot
//! stability tests (rows deleted/updated mid-scroll).

use ;
use ;

use ;
use ;
use ;

use shamir_db::access::{principal64, Actor};
use ;
use ;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use ;
use ;
use ;
use ;
use ;
use ;
use shamir_query_types::read::{
    OrderByReadQuery, Select,
};
use shamir_query_types::wire::{DbResponse};
use ;

use ;
use crate::db_handler::config::CursorLimitsCap;
use ;
use ;

use super::common::*;

// ---------------------------------------------------------------------------
// CR-C2 (#777) — cursor test-coverage gaps.
//
// `pagination_mode_for_query` picks `PaginationMode::Keyset` ONLY when the
// query has EXACTLY one `ORDER BY` item on a single (non-nested) field --
// every other shape (no `ORDER BY` at all, multi-column `ORDER BY`, a
// projection that excludes the sole ORDER BY column) falls back to
// `PaginationMode::Offset` (the row-count bookmark path). Every existing
// cursor test up to this point used a single-column `OrderBy::asc`/`desc`,
// so the offset fallback itself had zero DIRECT coverage of its own (the
// CR-B4 peek-ahead tests above exercise the offset branch too, but only for
// the has_more/exact-multiple edge, not "does full multi-page pagination
// with no ORDER BY at all ever lose or duplicate a row", multi-column
// ORDER BY, varying page_size mid-lifecycle, or concurrent FetchNext).
// ---------------------------------------------------------------------------

/// Gap 1: NO `order_by` at all, multiple pages -- draining the whole cursor
/// must surface every one of the original rows exactly once. This is the
/// most basic no-ORDER-BY pagination coverage this file has never had (the
/// existing no-ORDER-BY tests above are narrowly scoped to the has_more
/// peek-ahead edge case, not general multi-page correctness).
#[tokio::test]
async fn offset_path_no_order_by_full_pagination_every_row_once() {
    // 7 rows, page_size 3 -> pages of 3, 3, 1.
    let handler = build_handler_with_rows(7, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items"); // no .order_by(...) at all
    let resp = send(&handler, &session, create_cursor_req(query, 3)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 3, "first page must have page_size rows");
            assert!(has_more, "7 rows / page_size 3 -> more pages remain");
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
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 3)).await;
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
        (0..7).collect::<Vec<i64>>(),
        "every one of the 7 rows must appear exactly once across the whole \
         cursor lifetime with NO order_by at all (pages of 3, 3, 1)"
    );
}

/// Gap 2: a MULTI-COLUMN `ORDER BY` also falls back to the offset bookmark
/// (`pagination_mode_for_query` requires `items.len() == 1`), regardless of
/// whether the extra column is semantically inert -- draining the whole
/// cursor must still surface every row exactly once. `v` is unique per row
/// in this fixture, so `id` as a second sort key never actually breaks a
/// tie; the POINT of this test is that `ob.items.len() == 2` alone routes to
/// `PaginationMode::Offset`, which had no direct multi-column coverage.
#[tokio::test]
async fn offset_path_multi_column_order_by_falls_back_correctly() {
    let handler = build_handler_with_rows(7, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::new([
        shamir_query_types::read::OrderByItem::asc("v"),
        shamir_query_types::read::OrderByItem::asc("id"),
    ]));
    let resp = send(&handler, &session, create_cursor_req(query, 3)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 3, "first page must have page_size rows");
            assert!(has_more, "7 rows / page_size 3 -> more pages remain");
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
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 3)).await;
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
        (0..7).collect::<Vec<i64>>(),
        "a two-column ORDER BY (v asc, id asc) must still route through the \
         offset bookmark (items.len() != 1 -> PaginationMode::Offset) and \
         page every row exactly once, with no loss or duplication"
    );
}

/// Gap 3: `page_size` VARYING across `FetchNext` calls on the offset path --
/// `CreateCursor` with one page_size, then a sequence of `FetchNext` calls
/// each requesting a DIFFERENT explicit page_size. Every existing test uses
/// the SAME size for every call; this is a documented capability
/// (`FetchNext`'s `page_size` may differ per call -- CURSORS.md Sec.3) with
/// no prior test exercising an actual size CHANGE mid-lifecycle. Asserts
/// both that every row appears exactly once overall AND that each
/// individual page's row count matches what was actually requested (capped
/// by remaining data).
#[tokio::test]
async fn offset_path_varying_page_size_across_fetch_next_calls() {
    // 10 rows: CreateCursor(page_size=2), then FetchNext(5), FetchNext(1),
    // FetchNext(whatever's left).
    let handler = build_handler_with_rows(10, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items"); // no ORDER BY -> offset path
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 2, "CreateCursor page_size=2");
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

    // Page 2: FetchNext(page_size=5) -- 8 rows remain, so a full 5-row page
    // comes back with has_more still true.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 5)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(
                page.records.len(),
                5,
                "explicit page_size=5 must be honored exactly (8 rows remain, more than 5)"
            );
            assert!(has_more, "3 rows still remain after this page");
            for r in &page.records {
                seen_vs.push(r.get_value_i64("v").expect("v present"));
            }
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    // Page 3: FetchNext(page_size=1) -- 3 rows remain, request only 1.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 1)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(
                page.records.len(),
                1,
                "explicit page_size=1 must be honored exactly (2 rows still remain after)"
            );
            assert!(has_more, "2 rows still remain after this page");
            for r in &page.records {
                seen_vs.push(r.get_value_i64("v").expect("v present"));
            }
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    // Page 4: FetchNext(page_size=100, "whatever's left") -- only 2 rows
    // remain, capped by remaining data rather than the requested size.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 100)).await;
    match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert_eq!(
                page.records.len(),
                2,
                "requested page_size=100 must be capped by the 2 rows actually remaining"
            );
            assert!(!has_more, "last page must report has_more == false");
            for r in &page.records {
                seen_vs.push(r.get_value_i64("v").expect("v present"));
            }
        }
        other => panic!("expected CursorPage, got {other:?}"),
    }

    seen_vs.sort_unstable();
    assert_eq!(
        seen_vs,
        (0..10).collect::<Vec<i64>>(),
        "every one of the 10 rows must appear exactly once across a lifetime \
         where page_size CHANGED on every single FetchNext call (2, 5, 1, 100)"
    );
}

/// Gap 4: CONCURRENT `FetchNext` on the SAME cursor, from the same session.
/// `fetch_next`'s `state = cursor.state().lock().await` is a
/// `tokio::sync::Mutex` (see `cursor_registry.rs::Cursor::state`), held
/// across the whole internal read + bookmark-advance, so two concurrent
/// `FetchNext` calls against the SAME cursor are fully serialized by that
/// lock -- there is no partial-interleaving window where both calls could
/// observe (or advance from) the same `state.offset`. Whichever call
/// acquires the mutex first reads and advances the bookmark BEFORE the
/// second call's lock acquisition even begins its own read, so the two
/// calls always produce two DISJOINT, correctly-sequenced pages (page N and
/// page N+1) -- never overlapping, never skipping a row. This test asserts
/// exactly that observed, mutex-serialized property: the combined rows from
/// both concurrent calls have no duplicates and no losses relative to the
/// data remaining at that point in the cursor's lifecycle.
#[tokio::test]
async fn offset_path_concurrent_fetch_next_same_cursor_serializes_disjoint_pages() {
    let handler = build_handler_with_rows(8, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items"); // no ORDER BY -> offset path
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "8 rows / page_size 2 -> more pages remain");
            let vs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("v").expect("v present"))
                .collect();
            (cursor_id, vs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // 6 rows remain (v = 2..8). Fire two FetchNext(page_size=2) calls at
    // (as close to) the same time against the SAME cursor_id, same session.
    // The cursor's tokio::sync::Mutex serializes the two internal
    // read+bookmark-advance sequences, so this must produce two DISJOINT
    // 2-row pages covering 4 of the remaining 6 rows, with has_more == true
    // on both (since 2 rows still remain after either ordering).
    let fut_a = send(&handler, &session, fetch_next_req(cursor_id, 2));
    let fut_b = send(&handler, &session, fetch_next_req(cursor_id, 2));
    let (resp_a, resp_b) = tokio::join!(fut_a, fut_b);

    for resp in [resp_a, resp_b] {
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                assert_eq!(
                    page.records.len(),
                    2,
                    "each concurrent call must still get a full 2-row page \
                     (the mutex serializes them into two proper sequential pages)"
                );
                assert!(
                    has_more,
                    "4 of 8 consumed so far by these two pages -> more pages remain"
                );
                for r in &page.records {
                    seen_vs.push(r.get_value_i64("v").expect("v present"));
                }
            }
            other => panic!("expected CursorPage from a concurrent FetchNext, got {other:?}"),
        }
    }

    seen_vs.sort_unstable();
    assert_eq!(
        seen_vs,
        vec![0, 1, 2, 3, 4, 5],
        "the mutex-serialized concurrent calls must together cover exactly \
         the first page (v=0,1) plus the next 4 rows (v=2..6), no duplicates, \
         no losses -- proving two DISJOINT, correctly-sequenced pages, never \
         overlapping"
    );

    // Drain the rest sequentially to confirm the cursor's bookmark ended up
    // in a fully consistent state after the concurrent pair (not just that
    // the two concurrent pages individually looked right).
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
        (0..8).collect::<Vec<i64>>(),
        "after the concurrent pair plus draining the rest sequentially, all \
         8 rows must appear exactly once with no loss or duplication"
    );
}

/// Gap 5: an "unprojected-seek-field" query -- `ORDER BY v` (single column,
/// otherwise keyset-ELIGIBLE per `pagination_mode_for_query`, which only
/// inspects `order_by`, not `select`) but whose `SELECT` projection
/// EXCLUDES `v`. This drives `order_by_field_value` to return `None` (the
/// field isn't present on the projected `QueryRecord`), both at
/// `create_cursor` time (`page.records.last().and_then(|r|
/// order_by_field_value(&query, r))` -> `None`) and would at any subsequent
/// `fetch_next` call.
///
/// Traced behavior (verified by reading `create_cursor`/`fetch_next`, not
/// guessed): pre-F-1 (#792), `mode` was pinned to `PaginationMode::Keyset`
/// at creation (decided purely from `order_by`'s shape, per
/// `pagination_mode_for_query` -- `select` never enters that decision), and
/// `seek_key` ended up `None` because `order_by_field_value` can't extract
/// `v` from an unprojected row -- `fetch_next`'s `(Keyset, None)` dispatch
/// falls through to the SAME row-count offset-bookmark branch the
/// no-ORDER-BY path uses. Since F-1, `build_handler_with_rows`'s table has
/// no schema bound at all, so the NEW schema-typed-scalar gate ALSO now
/// pins `mode` to `PaginationMode::Offset` directly at creation (before the
/// unprojected-field question is even reached) -- a stricter, earlier
/// reason for the SAME degrade-to-offset outcome. Either mechanism lands on
/// the identical offset-bookmark branch (`state.offset` is tracked
/// identically regardless of which one triggered), so this test's
/// end-to-end assertion (every row pages correctly) is unaffected by which
/// mechanism is responsible. This is NOT a bug: it's the documented "can't
/// build/refresh a keyset bookmark from this page" fallback the module doc
/// comment on `order_by_field_value` describes, verified here to still page
/// every row correctly end-to-end.
#[tokio::test]
async fn keyset_eligible_but_unprojected_order_by_field_falls_back_to_offset_correctly() {
    let handler = build_handler_with_rows(7, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // ORDER BY v (single column -> keyset-eligible per pagination_mode_for_
    // query), but SELECT projects only "id" -- v is excluded from the
    // returned rows, so order_by_field_value can never extract a seek key.
    let query = ReadQuery::new("items")
        .select(Select::fields(["id"]))
        .order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 3)).await;
    let (cursor_id, mut seen_ids) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert_eq!(page.records.len(), 3, "first page must have page_size rows");
            assert!(has_more, "7 rows / page_size 3 -> more pages remain");
            // v is NOT projected -- confirm that directly, so this test
            // actually exercises the gap it claims to (not silently passing
            // because the projection accidentally still included v).
            for r in &page.records {
                assert!(
                    r.get_value_i64("v").is_none(),
                    "v must be excluded from the projection for this test to be meaningful"
                );
            }
            let ids: Vec<String> = page
                .records
                .iter()
                .map(|r| {
                    r.get_value("id")
                        .and_then(|v| v.as_str().map(str::to_string))
                        .expect("id present (it IS projected)")
                })
                .collect();
            (cursor_id, ids)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 3)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    assert!(
                        r.get_value_i64("v").is_none(),
                        "v must stay excluded from every subsequent page's projection too"
                    );
                    seen_ids.push(
                        r.get_value("id")
                            .and_then(|v| v.as_str().map(str::to_string))
                            .expect("id present"),
                    );
                }
                if !has_more {
                    break;
                }
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
    }

    seen_ids.sort_unstable();
    let expected: Vec<String> = (0..7).map(|i| format!("k{i:03}")).collect();
    assert_eq!(
        seen_ids, expected,
        "KNOWN GAP CHECKED, NOT A BUG: even though ORDER BY's field (v) is \
         excluded from the SELECT projection (order_by_field_value can never \
         extract a seek key, so seek_key stays None forever after creation), \
         fetch_next's (Keyset, None) dispatch falls through to the SAME \
         row-count offset-bookmark branch the no-ORDER-BY path uses -- every \
         row must still appear exactly once, in insertion order (id order == \
         v order == insertion order in this fixture), with no loss or \
         duplication"
    );
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
    del.try_delete(
        "d",
        shamir_query_builder::write::delete("items")
            .where_(shamir_query_builder::filter::eq("id", "k002")),
    )
    .unwrap();
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
    upd.try_update(
        "u",
        shamir_query_builder::write::update("items")
            .where_(shamir_query_builder::filter::eq("id", "k002"))
            .set(shamir_query_builder::doc! { "v" => 9999_i64 }),
    )
    .unwrap();
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
