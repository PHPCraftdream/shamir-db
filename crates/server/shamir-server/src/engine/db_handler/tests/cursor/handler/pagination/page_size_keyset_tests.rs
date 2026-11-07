//! Page-size validation/default tests plus CR-A4 keyset-pagination
//! (tie-run, bounded retry, ceiling transition, mode pinning) tests.

use shamir_query_builder::doc;
use shamir_query_types::read::{OrderBy, ReadQuery};
use shamir_query_types::wire::DbResponse;

use crate::db_handler::config::CursorLimitsCap;

use super::super::common::*;

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

/// CR-D1 (#782, release blocker): a tie run STRICTLY LARGER than
/// `max_cursor_page_size` must not livelock. Before the fix,
/// `fetch_keyset_page`'s ceiling branch could return `usable_len == 0`
/// (empty page, `has_more: true`) while recomputing the EXACT SAME
/// bookmark (`seek_key`/`tie_skip`) it started with — every subsequent
/// `FetchNext` against the cursor was then byte-identical forever, and
/// both SDKs' drain loops retry on exactly that shape (empty page +
/// `has_more: true`), hanging the consumer.
///
/// Reproduces with a SMALL `max_cursor_page_size` (8, via
/// `CursorLimitsCap`) and a tie run of 20 rows all sharing `score: 1` —
/// comfortably past the ceiling. Drains via a BOUNDED loop (`for _ in
/// 0..N`, N far larger than the number of pages a correct fix could ever
/// need) rather than `drain_seqs`'s unbounded `while has_more` — an
/// unbounded loop against the pre-fix code would hang this test (and the
/// whole suite) instead of failing it.
#[tokio::test]
async fn keyset_tie_run_exceeding_ceiling_does_not_livelock() {
    let scores = vec![1i64; 20];
    let cursor_limits = CursorLimitsCap {
        max_cursor_page_size: 8,
        ..CursorLimitsCap::UNLIMITED
    };
    let handler = build_handler_with_scores(&scores, cursor_limits).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 3)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "20 rows / page_size 3 -> more pages remain");
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Bounded drain: at most 20 more FetchNext calls could ever be needed
    // to exhaust 20 rows at page_size 3 (a generous upper bound, not a
    // tight one) -- this loop terminates on the CORRECT fix (offset
    // fallback or typed error) and would otherwise run to completion
    // WITHOUT ever observing `has_more == false`, which the assertions
    // below catch.
    const MAX_ROUNDS: usize = 20;
    let mut saw_terminal_page = false;
    let mut hit_tie_run_error = false;
    for round in 0..MAX_ROUNDS {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 3)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    seqs.push(
                        r.get_value_i64("seq")
                            .expect("every row must carry a seq field"),
                    );
                }
                if !has_more {
                    saw_terminal_page = true;
                    break;
                }
                // Livelock signature: an empty page with `has_more: true`
                // repeating forever. Fail fast with a clear message rather
                // than silently burning through `MAX_ROUNDS` iterations if
                // this regresses again.
                assert!(
                    !page.records.is_empty(),
                    "round {round}: empty page with has_more=true -- this is the CR-D1 \
                     livelock signature (stuck at the keyset ceiling with zero progress)"
                );
            }
            DbResponse::Error { code, .. } if code == "cursor_tie_run_too_large" => {
                // Acceptable minimum per the brief: a distinct, actionable
                // error instead of a silent infinite loop.
                hit_tie_run_error = true;
                break;
            }
            other => panic!("round {round}: expected CursorPage or a clean error, got {other:?}"),
        }
    }

    assert!(
        saw_terminal_page || hit_tie_run_error,
        "cursor must either fully drain (offset-fallback fix) or surface a clean, distinct \
         error (typed-error fallback) within {MAX_ROUNDS} rounds -- neither happened, which \
         means it livelocked exactly as CR-D1 describes"
    );

    if saw_terminal_page {
        seqs.sort_unstable();
        assert_eq!(
            seqs,
            (0..20).collect::<Vec<i64>>(),
            "every one of the 20 tied rows must appear exactly once once the cursor is fully \
             drained via the offset-mode fallback"
        );
    }
}

/// CR-D1 (#782): exactly-once correctness of the mode-transition itself.
/// A tie run SLIGHTLY over the ceiling (12 tied rows, cap 8) followed by 3
/// more DISTINCT-valued rows -- draining the whole cursor must return
/// every one of the 15 rows EXACTLY once, in the correct overall order,
/// proving the keyset -> offset mode switch neither duplicates nor skips
/// anything at the transition boundary (only meaningful when the
/// offset-fallback fix is landed; a typed-error fallback would fail this
/// test outright, which is an intentional signal of which path was taken).
#[tokio::test]
async fn keyset_ceiling_transition_to_offset_mode_is_exactly_once() {
    let mut scores = vec![9i64; 12];
    scores.push(10);
    scores.push(11);
    scores.push(12);

    let cursor_limits = CursorLimitsCap {
        max_cursor_page_size: 8,
        ..CursorLimitsCap::UNLIMITED
    };
    let handler = build_handler_with_scores(&scores, cursor_limits).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 4)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "15 rows / page_size 4 -> more pages remain");
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    drain_seqs(&handler, &session, cursor_id, 4, &mut seqs).await;

    let mut sorted = seqs.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (0..15).collect::<Vec<i64>>(),
        "all 15 rows (12-row tie run over the ceiling + 3 trailing distinct rows) must surface \
         exactly once -- proves the keyset->offset transition at the ceiling neither duplicated \
         nor skipped a row"
    );
    // The scores are non-decreasing by construction (score asc, `seq`
    // assigned in score order), so ordering by `seq` must be
    // non-decreasing too if overall ORDER BY order was preserved straight
    // through the mode transition. Since every score in the tie run is `9`
    // and identical to itself, and the trailing 3 rows are strictly
    // increasing distinct scores inserted after the tie run's `seq`
    // range, the returned order (before the final unstable sort above)
    // must already have been non-decreasing in `seq` across the tie-run
    // rows collectively followed by the trailing rows in increasing `seq`
    // order.
    let boundary = seqs
        .iter()
        .position(|&s| s == 12)
        .expect("seq 12 (first trailing distinct row) must be present");
    assert!(
        seqs[boundary..] == [12, 13, 14],
        "the 3 trailing distinct-valued rows must come out in correct ORDER BY order \
         (12, 13, 14) after the transition, got {:?}",
        &seqs[boundary..]
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
