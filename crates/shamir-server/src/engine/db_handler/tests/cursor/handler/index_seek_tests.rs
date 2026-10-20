//! F-53b sorted-index seek eligibility: pinned index seek for eligible
//! queries, permanent offset fallback after a concurrent write, page-3
//! stability, and DESC direction parity.

use std::sync::Arc;
use ;

use ;
use ;
use ;

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use ;
use ;
use ;
use ;
use ;
use shamir_query_types::read::{
    OrderByReadQuery,
};
use shamir_query_types::wire::{DbRequest, DbResponse};
use ;

use crate::cursor_registry::PaginationMode;
use crate::db_handler::config::CursorLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use ;

use super::common::*;

// ---------------------------------------------------------------------------

/// Build a handler over an in-memory `ShamirDb` with `app.main.items`, owned
/// by alice, seeded with one row per entry in `scores`: `{ seq: i, score:
/// scores[i] }`, with a SORTED INDEX on `score` (no schema binding — F-53b's
/// `probe_index_seek_eligible` only requires index coverage, not a schema
/// type gate, unlike `Keyset` mode's `order_by_column_is_schema_typed_scalar`).
async fn build_handler_with_sorted_index_scores(
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

    let table = shamir
        .get_table("app", "main", "items")
        .await
        .expect("get_table for index creation");
    table
        .create_sorted_index("score_idx", &["score"])
        .await
        .expect("create sorted index");

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

/// Positive + negative eligibility: `create_cursor` pins `IndexSeek` for a
/// plain `ORDER BY <indexed field>` with NO `WHERE`, and does NOT pin it
/// (falls through to the existing `Keyset`/`Offset` chain) for a query with
/// a `WHERE`, an unindexed ORDER BY column, or a multi-column ORDER BY —
/// mirrors the spike's `probe_1_eligibility_probe_uses_only_public_api`
/// intent, but driving the REAL `create_cursor`, not the test-local probe.
#[tokio::test]
async fn create_cursor_pins_index_seek_for_eligible_query_not_for_ineligible() {
    let handler =
        build_handler_with_sorted_index_scores(&[0, 10, 20, 30, 40], CursorLimitsCap::UNLIMITED)
            .await;
    let session = alice_session();

    // Eligible: no WHERE, single indexed ORDER BY column.
    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
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
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "a plain ORDER BY over an indexed column with no WHERE must pin IndexSeek"
    );
    send(&handler, &session, DbRequest::CancelCursor { cursor_id }).await;

    // Ineligible: a WHERE clause on the same indexed column disqualifies
    // IndexSeek entirely (try_plan_keyset_seek's shared where.is_some()
    // guard) -- falls through to the existing Keyset chain instead.
    let mut query_where = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    query_where.r#where = Some(shamir_query_types::filter::Filter::Gte {
        field: shamir_query_types::filter::FieldPath::from(vec!["score".to_string()]),
        value: shamir_query_types::filter::FilterValue::Int(0),
    });
    let resp = send(&handler, &session, create_cursor_req(query_where, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_ne!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "a caller WHERE clause must disqualify IndexSeek"
    );
    send(&handler, &session, DbRequest::CancelCursor { cursor_id }).await;

    // Ineligible: ORDER BY on a column with no sorted index.
    let query_unindexed = ReadQuery::new("items").order_by(OrderBy::asc("seq"));
    let resp = send(&handler, &session, create_cursor_req(query_unindexed, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_ne!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "ORDER BY on an unindexed column must not pin IndexSeek"
    );
    send(&handler, &session, DbRequest::CancelCursor { cursor_id }).await;

    // Ineligible: multi-column ORDER BY (no composite seek primitive).
    let mut query_multi = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    query_multi.order_by = Some(shamir_query_types::read::OrderBy {
        items: vec![
            shamir_query_types::read::OrderByItem {
                field: vec!["score".to_string()],
                direction: shamir_query_types::read::OrderDirection::Asc,
                nulls: None,
            },
            shamir_query_types::read::OrderByItem {
                field: vec!["seq".to_string()],
                direction: shamir_query_types::read::OrderDirection::Asc,
                nulls: None,
            },
        ],
    });
    let resp = send(&handler, &session, create_cursor_req(query_multi, 2)).await;
    let cursor_id = match resp {
        DbResponse::CursorPage { cursor_id, .. } => cursor_id,
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_ne!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "multi-column ORDER BY must not pin IndexSeek"
    );
}

/// THE load-bearing negative test, ported from the spike's
/// `negative_gate_failure_mid_lifetime_must_not_page_one_forever` to drive
/// the REAL `fetch_next`: `CreateCursor` (no WHERE, indexed ORDER BY) ->
/// `FetchNext` page 1 (assert `IndexSeek`, seek arm fired) -> concurrent
/// write -> `FetchNext` page 2 (assert transparent fallback to `Offset`,
/// CORRECT data -- not a duplicate of page 1, mode is now permanently
/// `Offset`) -> `FetchNext` page 3 (assert it STAYS on `Offset`, no attempt
/// to re-probe `IndexSeek`).
#[tokio::test]
async fn index_seek_falls_back_to_offset_on_concurrent_write_and_stays_there() {
    // 40 rows: scores 0,10,...,390 -> 4 pages of 10 (page1=[0..90]). Using 4
    // pages (not 3) so the gate-decline page (page 3) still has `has_more ==
    // true` -- letting the mode assertion run BEFORE the cursor's eventual
    // auto-close on the final exhausted page (page 4), instead of racing it.
    let scores: Vec<i64> = (0..40).map(|i| i * 10).collect();
    let handler = build_handler_with_sorted_index_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 10)).await;
    let (cursor_id, page1_scores) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "40 rows / page_size 10 -> more pages remain");
            let vals: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect();
            (cursor_id, vals)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "eligible query must pin IndexSeek at CreateCursor time"
    );
    assert_eq!(
        page1_scores,
        (0..10).map(|i| i * 10).collect::<Vec<_>>(),
        "page 1 (from CreateCursor) must be scores [0..90]"
    );

    // FetchNext page 1 (the cursor's SECOND page overall, first via
    // fetch_next): must still take the seek arm (no concurrent write yet).
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 10)).await;
    let page2_scores = match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(has_more, "20 of 40 consumed -> more pages remain");
            page.records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect::<Vec<_>>()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "no gate failure yet -- mode must still be IndexSeek"
    );
    assert_eq!(
        page2_scores,
        (10..20).map(|i| i * 10).collect::<Vec<_>>(),
        "page 2 (first real FetchNext) must be scores [100..190], continuing from page 1"
    );

    // Concurrent write: UPDATE the indexed field on an UNRELATED row (the
    // row at score 300, seq 30), landing strictly between this FetchNext and
    // the next one -- trips the F-53b Step 2 last_mutation_version gate for
    // the WHOLE index (a one-way ratchet), same scenario the spike proved.
    let owner = Actor::User(principal64([0xAB; 16]));
    let mut update_batch = Batch::new();
    update_batch
        .try_update(
            "mv",
            shamir_query_builder::write::update("items")
                .where_(shamir_query_builder::filter::eq("seq", 30i64))
                .set(shamir_query_builder::doc! { "score" => 305i64 }),
        )
        .unwrap();
    handler
        .db()
        .execute_as(owner, "app", &update_batch.build())
        .await
        .expect("concurrent update must commit");

    // FetchNext page 3: the gate has now declined -- must transparently fall
    // back to Offset mode and return the CORRECT next page ([200..290]), not
    // a duplicate of page 1 or page 2. `has_more` is still `true` here (10 of
    // 40 rows remain), so the cursor is NOT auto-closed yet -- the mode
    // assertion right after can still observe it.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 10)).await;
    let page3_scores = match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(has_more, "30 of 40 consumed -> one more page remains");
            page.records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect::<Vec<_>>()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "a gate decline mid-lifetime must permanently flip the cursor to Offset mode"
    );
    assert_eq!(
        page3_scores,
        (20..30).map(|i| i * 10).collect::<Vec<_>>(),
        "the offset-mode fallback must return the TRUE next page ([200..290]), not repeat \
         an earlier page -- proves the mandatory fallback, not a naive always-After scheme"
    );
    assert!(
        !page3_scores.contains(&305),
        "the pinned snapshot must not observe the post-pin UPDATE's new value"
    );

    // FetchNext page 4: the final page, exhausting the cursor -- confirms
    // the fallback's data continuity holds to the very end.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 10)).await;
    let page4_scores = match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(
                !has_more,
                "all 40 rows consumed across 4 pages -> has_more must be false"
            );
            page.records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect::<Vec<_>>()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        page4_scores,
        (30..40).map(|i| i * 10).collect::<Vec<_>>(),
        "page 4 must be the true final page [300..390]"
    );

    // Cursor is now exhausted and auto-closed (has_more was false above).
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "the fully-drained cursor must have been auto-closed"
    );
}

/// New test (this task): the `Offset` fallback holds PERMANENTLY -- once
/// tripped, a cursor never attempts to re-probe `IndexSeek` on a LATER page,
/// even across multiple further `FetchNext` calls. Uses a smaller page size
/// so there are enough pages after the fallback to prove "stays flipped",
/// not just "flips once".
#[tokio::test]
async fn index_seek_offset_fallback_holds_permanently_across_page_3() {
    // 25 rows: scores 0,10,...,240 -> pages of 5: [0..40],[50..90],
    // [100..140],[150..190],[200..240]. 5 pages (not 4) so page 4's
    // `has_more` is still `true` -- the mode assertion right after it can
    // still observe the (not-yet-auto-closed) cursor.
    let scores: Vec<i64> = (0..25).map(|i| i * 10).collect();
    let handler = build_handler_with_sorted_index_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 5)).await;
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
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek
    );

    // Page 2 (first fetch_next): still IndexSeek, no concurrent write yet.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 5)).await;
    assert!(matches!(resp, DbResponse::CursorPage { has_more, .. } if has_more));
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek
    );

    // Concurrent write trips the gate.
    let owner = Actor::User(principal64([0xAB; 16]));
    let mut update_batch = Batch::new();
    update_batch
        .try_update(
            "mv",
            shamir_query_builder::write::update("items")
                .where_(shamir_query_builder::filter::eq("seq", 20i64))
                .set(shamir_query_builder::doc! { "score" => 205i64 }),
        )
        .unwrap();
    handler
        .db()
        .execute_as(owner, "app", &update_batch.build())
        .await
        .expect("concurrent update must commit");

    // Page 3: gate declines -> falls back to Offset (asserted by the
    // sibling test above in detail; here we only need the mode transition).
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 5)).await;
    let page3_scores: Vec<i64> = match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(has_more, "15 of 25 consumed -> more pages remain");
            page.records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "page 3 must trip the fallback"
    );
    assert_eq!(page3_scores, (10..15).map(|i| i * 10).collect::<Vec<_>>());

    // Page 4: must STAY on Offset -- no attempt to re-probe IndexSeek, even
    // though this is a later page with no NEW concurrent write since the
    // fallback. If IndexSeek were re-probed and happened to succeed, that
    // would violate the "decided once, permanent" contract this task's
    // PaginationMode doc comment states. `has_more` is still `true` here (5
    // of 25 rows remain), so the cursor is NOT auto-closed yet.
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 5)).await;
    let page4_scores: Vec<i64> = match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(has_more, "20 of 25 consumed -> one more page remains");
            page.records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "the fallback must hold PERMANENTLY -- page 4 must NOT re-probe IndexSeek"
    );
    assert_eq!(page4_scores, (15..20).map(|i| i * 10).collect::<Vec<_>>());

    // Page 5: the final page, confirming exhaustion and data continuity to
    // the very end -- no trailing mode-check here (an exhausted cursor is
    // auto-closed by fetch_next, so there is nothing left to inspect).
    let resp = send(&handler, &session, fetch_next_req(cursor_id, 5)).await;
    let page5_scores: Vec<i64> = match resp {
        DbResponse::CursorPage { page, has_more, .. } => {
            assert!(!has_more, "all 25 rows consumed across 5 pages");
            page.records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect()
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(page5_scores, (20..25).map(|i| i * 10).collect::<Vec<_>>());
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "the fully-drained cursor must have been auto-closed"
    );
}

/// DESC-direction parity: the seek arm already supports it (F-53b Step 2);
/// the cursor wiring must carry it through without a separate code path --
/// `IndexSeek` is pinned the same way, and pages come back in descending
/// order across the cursor's whole lifetime with no concurrent write.
#[tokio::test]
async fn index_seek_desc_direction_parity() {
    // 20 rows: scores 0,10,...,190.
    let scores: Vec<i64> = (0..20).map(|i| i * 10).collect();
    let handler = build_handler_with_sorted_index_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::desc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 5)).await;
    let (cursor_id, page1_scores) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more);
            let vals: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("score").expect("score present"))
                .collect();
            (cursor_id, vals)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::IndexSeek,
        "DESC ORDER BY over an indexed column with no WHERE must also pin IndexSeek"
    );
    assert_eq!(
        page1_scores,
        (15..20).rev().map(|i| i * 10).collect::<Vec<_>>(),
        "page 1 DESC must be the 5 highest scores, descending: [190,180,170,160,150]"
    );

    let mut all_scores = page1_scores;
    loop {
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 5)).await;
        match resp {
            DbResponse::CursorPage { page, has_more, .. } => {
                for r in &page.records {
                    all_scores.push(r.get_value_i64("score").expect("score present"));
                }
                if !has_more {
                    // The final, exhausted page auto-closes the cursor
                    // (`fetch_next` removes it from the registry once
                    // `has_more == false`) -- nothing left to inspect via
                    // `pinned_mode` after this point.
                    break;
                }
                assert_eq!(
                    pinned_mode(&handler, cursor_id, &ALICE_SID),
                    PaginationMode::IndexSeek,
                    "no concurrent write in this test -- must stay on IndexSeek every page"
                );
            }
            other => panic!("expected CursorPage while draining DESC cursor, got {other:?}"),
        }
    }

    assert_eq!(
        all_scores,
        (0..20).rev().map(|i| i * 10).collect::<Vec<_>>(),
        "every row exactly once, in strict descending order, across the whole cursor lifetime"
    );
}
