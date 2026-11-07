//! CR-D2 (#783) Null/missing ORDER BY value tests: null/missing/NaN
//! ORDER BY columns must fall back to offset mode without losing rows.

use shamir_query_types::read::{OrderBy, ReadQuery};
use std::sync::Arc;

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::filter::FilterValue;
use shamir_query_types::wire::DbResponse;

use crate::cursor_registry::PaginationMode;
use crate::db_handler::config::CursorLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;

use super::super::common::*;

// ---------------------------------------------------------------------------
// CR-D2 (#783, release blocker) — keyset cursor silently loses Null/missing
// ORDER BY rows.
//
// Root cause: the keyset bookmark's boundary filter is `field >= seek_key`
// (ASC), evaluated through `compare_values` (`shamir-engine`'s
// `query/filter/resolve.rs`). Any row whose ORDER BY value cannot be
// compared to `seek_key` (a `Null`/missing field vs. any real seek value)
// makes the comparison return `None` -> the filter is `false` for that row.
// Meanwhile `QvSortKey`'s sort (`shamir-engine`'s `query/read/order.rs`)
// places `Null` LAST under the ASC default -- so page 1 (no boundary yet)
// returns only the leading real-valued rows, and EVERY later page's
// `field >= seek` boundary permanently excludes the null/missing rows. The
// scan looks like it "ran out" (`has_more: false`), but those rows were
// never returned at all.
//
// Fix (MUST-fix tier, per the brief): at `create_cursor` time, once
// `pagination_mode_for_query` has already decided `Keyset`, run one cheap
// `WHERE <order_by_field> IS NULL LIMIT 1` existence probe at the SAME
// pinned snapshot. If it finds any row, the WHOLE cursor falls back to
// `PaginationMode::Offset` from creation -- closing the null/missing case
// unconditionally (a null-containing ORDER BY column can never reach the
// keyset boundary-filter bug again).
// ---------------------------------------------------------------------------

/// Build `app.main.items` with a `score` (ORDER BY) column where SOME rows
/// have an explicit `Null` score and/or omit the field entirely (both cases
/// `Filter::IsNull`/`is_null_at` treat identically -- see
/// `RecordRef::is_null_at`'s `matches!(.., None | Some(Kind::Null))`).
/// `seq` is always present so drained rows can be identified uniquely.
async fn build_handler_with_nullable_scores(
    scores: &[Option<i64>],
    cursor_limits: CursorLimitsCap,
) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    if !scores.is_empty() {
        let mut b = Batch::new();
        for (i, score) in scores.iter().enumerate() {
            let row = match score {
                Some(s) => doc! { "seq" => i as i64, "score" => *s },
                // Explicit Null (as opposed to a genuinely absent field) --
                // both must trip the same IS NULL existence probe.
                None => doc! { "seq" => i as i64, "score" => FilterValue::Null },
            };
            b.insert(format!("i{i}"), insert("items").row(row));
        }
        let batch = b.build();
        shamir
            .execute_as(owner, "app", &batch)
            .await
            .expect("seed rows");
    }

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

/// Null ORDER BY value, ASC: 3 rows with real scores (0, 1, 2) and 2 rows
/// with a Null score. Before the fix: page 1 returns the 3 real-valued rows
/// (Null sorts last under ASC, so they're never on page 1 when page_size <
/// total), then every subsequent `field >= seek_key` boundary excludes the
/// Null rows forever -- the cursor reports `has_more: false` having silently
/// dropped both Null rows. After the fix: the existence probe detects the
/// Null score at `create_cursor` time and pins the WHOLE cursor to
/// `PaginationMode::Offset`, so every row (real-valued AND Null) surfaces
/// exactly once, regardless of which mode ultimately serves them.
#[tokio::test]
async fn null_order_by_value_asc_is_not_silently_dropped() {
    let scores = [Some(0), Some(1), Some(2), None, None];
    let handler = build_handler_with_nullable_scores(&scores, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // A null-containing ORDER BY column must be detected at CreateCursor
    // time and pinned to Offset for its WHOLE lifetime -- never Keyset,
    // which is exactly the mode that silently drops these rows.
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "a Null-containing ORDER BY column must fall back to PaginationMode::Offset \
         at CreateCursor time (the cheap IS NULL existence probe must have detected it)"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row -- including BOTH Null-score rows (seq 3, 4) -- must appear exactly once; \
         before the fix, the Null rows vanished silently with a clean has_more: false"
    );
}

/// Missing field (never set at all, as opposed to an explicit `Null`) must
/// trip the SAME existence probe and fall back the SAME way --
/// `Filter::IsNull`/`RecordRef::is_null_at` treat "absent" and "explicit
/// Null" identically by design, and this test pins that the create-time
/// probe actually relies on that (a probe that only checked "is the field
/// present and Null" but not "is it plain absent" would miss this case).
#[tokio::test]
async fn missing_order_by_field_is_not_silently_dropped() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // 3 rows with a real `score`, 2 rows that never set `score` at all.
    let mut b = Batch::new();
    for i in 0..3 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64, "score" => i as i64 }),
        );
    }
    for i in 3..5 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64 }), // no "score" key at all
        );
    }
    let batch = b.build();
    shamir
        .execute_as(owner, "app", &batch)
        .await
        .expect("seed rows");

    let handler =
        ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED);
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "a missing (never-set) ORDER BY field must ALSO fall back to \
         PaginationMode::Offset -- same probe, same IS NULL semantics as explicit Null"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row -- including both rows that never set score at all (seq 3, 4) -- \
         must appear exactly once"
    );
}

/// Regression: a uniformly-typed, non-null ORDER BY column must stay on
/// `PaginationMode::Keyset` (the existence probe must NOT false-positive and
/// degrade every cursor to Offset by mistake), and existing keyset behavior
/// (tie-run handling included) must remain intact.
#[tokio::test]
async fn null_probe_regression_non_null_column_stays_keyset() {
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

    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Keyset,
        "a non-null, uniformly-typed ORDER BY column must NOT be routed through the new \
         Offset-fallback probe -- this fix must not silently degrade every cursor to offset mode"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3],
        "regression: the existing tie-run scenario must still return every row exactly once \
         under genuine Keyset mode"
    );
}

/// NaN in an `F64` `ORDER BY` column: documented, still-open limitation for
/// the KEYSET boundary-filter scheme itself (see CURSORS.md and
/// KNOWN_LIMITATIONS.md §6) -- `NaN`'s `partial_cmp` always returns `None`
/// (`compare_values`'s `(F64, F64)` arm), so `field >= seek_key` would
/// silently drop a NaN row past page 1 if this column ever reached keyset
/// mode. F-1 (#792) does not (and cannot, without a new NaN-detection
/// primitive -- out of scope) close this directly; instead it closes it
/// INDIRECTLY for every `F64` column, schema-typed or not: an `F64` `ORDER BY`
/// column can no longer reach keyset mode AT ALL (this table is schemaless,
/// so `order_by_column_is_schema_typed_scalar` already returns `false` before
/// any schema-type check even applies -- see the schema-typed `F64`-exclusion
/// regression test below for the schema-bound case specifically). This test
/// now pins the NEW behavior: `PaginationMode::Offset` from creation, and
/// NO row loss (a plain offset-bookmarked full scan never runs `NaN` through
/// `compare_values`'s boundary-filter comparison at all).
#[tokio::test]
async fn nan_order_by_value_forces_offset_mode_no_loss() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // 3 real-valued rows (0.0, 1.0, 2.0) + 2 NaN-valued rows. Before F-1,
    // NaN's `partial_cmp` always returning `None` (`compare_values`'s
    // `(F64, F64) => a.partial_cmp(b)` arm) made `field >= seek_key`
    // evaluate to `false` for a NaN row on every page past the first --
    // silent loss under Keyset mode. After F-1, this schemaless `F64` column
    // never reaches Keyset mode at all.
    let mut b = Batch::new();
    for i in 0..3 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64, "score" => i as f64 }),
        );
    }
    for i in 3..5 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64, "score" => f64::NAN }),
        );
    }
    let batch = b.build();
    shamir
        .execute_as(owner, "app", &batch)
        .await
        .expect("seed rows");

    let handler =
        ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED);
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs, mut has_more) = match resp {
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
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // F-1 (#792): a schemaless `F64` ORDER BY column must never reach
    // Keyset mode -- the schema-typed-scalar gate rejects it (no bound
    // schema at all) BEFORE the pre-existing Null-only existence probe
    // even runs, closing the NaN silent-loss gap indirectly by removing
    // this column's access to the keyset boundary-filter scheme entirely.
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "an F64 ORDER BY column (schemaless here) must fall back to PaginationMode::Offset \
         from creation -- F-1's schema-typed-scalar gate never admits F64 columns into \
         Keyset mode, regardless of schema, since NaN cannot be ruled out"
    );

    // Bounded drain -- offset-mode pagination cannot livelock; bounded anyway
    // as cheap insurance.
    for _ in 0..10 {
        if !has_more {
            break;
        }
        let resp = send(&handler, &session, fetch_next_req(cursor_id, 2)).await;
        match resp {
            DbResponse::CursorPage {
                page, has_more: hm, ..
            } => {
                for r in &page.records {
                    seqs.push(
                        r.get_value_i64("seq")
                            .expect("every row must carry a seq field"),
                    );
                }
                has_more = hm;
            }
            other => panic!("expected CursorPage, got {other:?}"),
        }
    }

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "under Offset mode, every row -- including BOTH NaN-score rows -- must appear exactly \
         once; offset pagination never runs a NaN value through compare_values's boundary-filter \
         comparison, so the pre-F-1 silent-loss failure mode cannot occur here"
    );
}

// ---------------------------------------------------------------------------
// W-2 + W-3 (#789, Wave D review, https://github.com/, findings W-2/W-3) --
// keyset cursor bookmark typing gaps for `Bin`/`List` (W-2) and `Dec`/`Big`
// (W-3) ORDER BY columns.
//
// Root cause (shared with CR-D2): `seek_key` was set from whatever raw
// `QueryValue` the ORDER BY column held, with no check that this specific
// VALUE can round-trip through `boundary_filter`'s `query_value_to_filter_value`
// call AND actually be matched by `compare_values`.
//
// - W-3 (`Dec`/`Big`): `query_value_to_filter_value` returns `None` for these
//   (no `FilterValue` equivalent) -- BEFORE the fix, `boundary_filter` itself
//   returned `None` and `fetch_keyset_page`'s
//   `let Some(filter) = boundary_filter(..) else { return Err(..) }` hard-
//   errored ("cursor: keyset seek key has no comparable filter form") on every
//   `FetchNext` past page 1.
// - W-2 (`Bin`/`List`): these DO convert to a `FilterValue`, but
//   `compare_values` has no `(Bin, Bin)`/`(List, List)` comparison arm --
//   BEFORE the fix, the boundary filter silently matched nothing past page 1
//   (the CR-D2 failure shape: `has_more: false`, no error, rows just vanish).
//
// Fix (this task): `safe_seek_key` (`cursor_handlers.rs`) filters a candidate
// bookmark value through `query_value_to_filter_value`'s convertibility AND
// explicitly excludes `Bin`/`List` (chosen over extending `compare_values`
// with a real `Bin`/`List` total order -- see `safe_seek_key`'s doc comment
// for why: `order.rs`'s `QvSortKey` does NOT already solve this, since
// `Bin`/`List` map to the explicitly-unsortable `QvSortKey::Other` bucket
// today, so there is no existing total order to reuse/mirror). Either way,
// `None` routes into the SAME pre-existing "no seek_key" safety net a
// Keyset-mode cursor already has: fall back to the row-count offset bookmark
// for that call.
// ---------------------------------------------------------------------------
