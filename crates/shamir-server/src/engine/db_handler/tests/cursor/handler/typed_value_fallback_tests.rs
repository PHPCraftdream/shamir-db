//! ORDER BY value types the keyset boundary cannot compare (Dec, big
//! int, binary, list, str/bool/int/f64 regressions, schema-typed gates):
//! each must fall back to offset mode or use a real keyset seek, without
//! loss or hard errors.

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
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_query_types::read::{
    OrderByReadQuery,
};
use shamir_query_types::wire::{DbResponse};
use shamir_types::types::value::QueryValue;

use crate::cursor_registry::PaginationMode;
use crate::db_handler::config::CursorLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use ;

use super::common::*;

/// Build `app.main.items` with a `score` (ORDER BY) column of type `Dec`
/// (`rust_decimal::Decimal`), via `Doc::set_value` (the only way to inject a
/// `QueryValue::Dec` into a row -- `FilterValue`, which `doc!`'s `.set()`
/// targets, has no `Dec` variant at all). `seq` is always present so drained
/// rows can be identified uniquely.
async fn build_handler_with_dec_scores(
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

    let mut b = Batch::new();
    for (i, score) in scores.iter().enumerate() {
        let row =
            doc! { "seq" => i as i64 }.set_value("score", QueryValue::Dec(Decimal::from(*score)));
        b.insert(format!("i{i}"), insert("items").row(row));
    }
    let batch = b.build();
    shamir
        .execute_as(owner, "app", &batch)
        .await
        .expect("seed rows");

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

/// Same as [`build_handler_with_dec_scores`] but for `Big`
/// (`num_bigint::BigInt`) -- large enough to force the `Big` promotion (any
/// `i64` literal already round-trips through `QueryValue::Big` directly here
/// since the row is built by hand, not via wire-encoded literal promotion).
async fn build_handler_with_big_scores(
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

    let mut b = Batch::new();
    for (i, score) in scores.iter().enumerate() {
        let row =
            doc! { "seq" => i as i64 }.set_value("score", QueryValue::Big(BigInt::from(*score)));
        b.insert(format!("i{i}"), insert("items").row(row));
    }
    let batch = b.build();
    shamir
        .execute_as(owner, "app", &batch)
        .await
        .expect("seed rows");

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

/// Same as [`build_handler_with_dec_scores`] but for `Bin` (`Vec<u8>`) --
/// each row's `score` is a distinct single-byte binary value so an ASC keyset
/// scan (if it worked) would have a well-defined order.
///
/// F-1 (#792): binds a schema declaring `score: Bin, required` -- at the
/// time, `Bin` was (mistakenly) one of the accepted `TypeTag`s in the
/// schema-typed-scalar gate, so this kept the cursor eligible for `Keyset`
/// mode, exercising the W-2 per-value mechanism (`safe_seek_key` excluding
/// `Bin` unconditionally, since `compare_values` has no `(Bin, Bin)`
/// comparison arm) instead of being short-circuited to `Offset` before that
/// mechanism was ever reached.
///
/// F-18 (#811): `Bin` is now EXCLUDED from the gate's accepted `TypeTag` set
/// too -- `compare_values` never had a `(Bin, Bin)` arm, so a `Bin` column
/// could never actually benefit from Keyset mode; it only paid for the
/// null-probe read before degrading to the offset fallback on every call.
/// The schema binding is kept here (rather than dropped) so this test still
/// proves the exclusion holds even for a `keyset_safe: true`, schema-typed
/// `Bin` column -- not just a schemaless one.
async fn build_handler_with_bin_scores(
    scores: &[u8],
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
    bind_schema(
        &shamir,
        "app",
        "main",
        "items",
        "score",
        shamir_db::engine::validator::schema::TypeTag::Bin,
    )
    .await;

    let mut b = Batch::new();
    for (i, score) in scores.iter().enumerate() {
        let row = doc! { "seq" => i as i64 }.set_value("score", QueryValue::Bin(vec![*score]));
        b.insert(format!("i{i}"), insert("items").row(row));
    }
    let batch = b.build();
    shamir
        .execute_as(owner, "app", &batch)
        .await
        .expect("seed rows");

    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(cursor_limits)
}

/// W-3 regression, updated for F-1 (#792): a cursor over a `Dec` ORDER BY
/// column.
///
/// Pre-F-1 behavior (still exercised by the mechanism itself, just no
/// longer reachable from a schemaless setup): BEFORE W-3's fix,
/// `query_value_to_filter_value` returning `None` for `QueryValue::Dec` made
/// `boundary_filter` return `None` and `fetch_keyset_page` hard-error
/// ("cursor: keyset seek key has no comparable filter form") on the second
/// `FetchNext`. AFTER W-3's fix (unchanged by this task): `safe_seek_key`
/// would detect the unconvertible `Dec` value and degrade `seek_key` to
/// `None`, riding the offset-bookmark safety net while `state.mode` stayed
/// `Keyset` (pinned from shape).
///
/// F-1 (#792) changes what actually gates this: `Dec` has no accepted
/// `TypeTag` in the schema-typed-scalar gate (only `Int`/`Bool`/`String`
/// qualify -- `Bin` also qualified at F-1 time, but was excluded too by
/// F-18 (#811)), so a `Dec` ORDER BY column can never reach `Keyset` mode
/// at all anymore, schema-bound or not -- the gate now short-circuits the
/// whole keyset attempt BEFORE `safe_seek_key`'s per-value mechanism would
/// ever run. This test now pins `PaginationMode::Offset` from creation
/// (rather than the old "Keyset shape, but `seek_key` empties per-call"
/// story) -- the end-to-end no-loss, no-hard-error guarantee is unchanged.
#[tokio::test]
async fn dec_order_by_value_now_forces_offset_mode_no_hard_error() {
    let scores = [5, 3, 1, 4, 2];
    let handler = build_handler_with_dec_scores(&scores, CursorLimitsCap::UNLIMITED).await;
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
        "F-1 (#792): Dec has no accepted TypeTag in the schema-typed-scalar gate, so this \
         schemaless Dec column must fall back to PaginationMode::Offset from creation -- \
         W-3's per-value safe_seek_key fallback is no longer reachable here at all"
    );

    // Draining must NOT hard-error -- before W-3's fix, this loop's second
    // `FetchNext` would have returned `DbResponse::Error` instead of a
    // `CursorPage`. That guarantee still holds under Offset mode.
    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once via offset-mode pagination -- no hard error, no loss"
    );
}

/// W-3 regression, updated for F-1 (#792): same as the `Dec` test above, but
/// for `Big` (`num_bigint::BigInt`) -- `Big` has no `TypeTag` variant at all
/// (checked `type_tag.rs`), so it can never pass the schema-typed-scalar
/// gate under ANY schema, exactly like `Dec`.
#[tokio::test]
async fn big_order_by_value_now_forces_offset_mode_no_hard_error() {
    let scores = [5, 3, 1, 4, 2];
    let handler = build_handler_with_big_scores(&scores, CursorLimitsCap::UNLIMITED).await;
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
        "F-1 (#792): Big has no TypeTag at all, so this schemaless Big column must fall \
         back to PaginationMode::Offset from creation"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once via offset-mode pagination -- no hard error, no loss"
    );
}

/// W-2 regression, updated for F-18 (#811): a keyset-requesting cursor over
/// a `Bin` ORDER BY column must ride the offset-mode bookmark (every row
/// exactly once), not the OLD silent drop.
///
/// BEFORE W-2's fix: `Bin` converts fine to `FilterValue::Binary`, so
/// `boundary_filter` built a filter that LOOKED valid, but `compare_values`
/// has no `(Bin, Bin)` arm -- the filter matched nothing past page 1, and the
/// cursor reported a clean `has_more: false` having silently dropped every
/// remaining row. AFTER W-2's fix (still true, but no longer reachable from
/// here): `safe_seek_key` would explicitly exclude `Bin`, so `seek_key` was
/// `None` from the first page onward while `state.mode` stayed `Keyset`
/// (pinned from shape) -- a per-CALL fallback.
///
/// AFTER F-18 (#811, this task): `Bin` is excluded from the schema-typed-
/// scalar GATE itself (`order_by_column_is_schema_typed_scalar`'s accepted
/// set narrowed to `Int | Bool | String`), since `compare_values` never had
/// a `(Bin, Bin)` arm and a `Bin` column could therefore never actually
/// benefit from Keyset mode -- it only paid for the null-probe read before
/// degrading anyway. So this schema-typed `Bin` column now pins
/// `PaginationMode::Offset` from `create_cursor` time, same as `Dec`/`Big`/
/// `List` above -- W-2's per-call `safe_seek_key` mechanism is no longer
/// reached at all for `Bin`, made moot by the gate-level exclusion. The
/// end-to-end no-loss guarantee (every row exactly once, no silent drop) is
/// unchanged.
#[tokio::test]
async fn bin_order_by_value_now_forces_offset_mode_no_silent_drop() {
    let scores: [u8; 5] = [50, 30, 10, 40, 20];
    let handler = build_handler_with_bin_scores(&scores, CursorLimitsCap::UNLIMITED).await;
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
        "F-18 (#811): Bin is now excluded from the schema-typed-scalar gate's accepted \
         TypeTag set (compare_values never had a (Bin, Bin) arm, so Keyset mode could never \
         actually benefit this column) -- this schema-typed Bin column must fall back to \
         PaginationMode::Offset from creation, same as Dec/Big/List, not ride a per-call \
         seek_key fallback under a Keyset-pinned mode"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once via offset-mode pagination -- before W-2's fix \
         every row past page 1 was silently dropped with a clean has_more: false, the exact \
         CR-D2 failure shape; the gate-level exclusion (F-18) preserves that no-loss guarantee \
         by a different mechanism"
    );
}

/// W-2 regression, `List` variant, updated for F-1 (#792): same scenario as
/// `Bin` above, but for `QueryValue::List` (converts to `FilterValue::Array`
/// fine, but `compare_values` has no `(List, List)` arm either). `List` is a
/// CONTAINER type -- `TypeTag::List` is excluded from the schema-typed-scalar
/// gate's accepted set regardless of schema (containers can hold
/// heterogeneous/nested values, so schema enforcement of the outer type
/// alone doesn't prove the boundary-filter comparison is safe) -- so this
/// column can never reach `Keyset` mode at all, schema-bound or not, same as
/// the `Dec`/`Big` tests above (and, since F-18 (#811), the `Bin` test above
/// too -- though `Bin` is excluded for a different reason: a missing
/// `compare_values` arm on an otherwise fixed, non-container scalar, not
/// because it's a container).
#[tokio::test]
async fn list_order_by_value_now_forces_offset_mode_no_silent_drop() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    let mut b = Batch::new();
    for i in 0..5i64 {
        let row = doc! { "seq" => i }.set_value(
            "score",
            QueryValue::List(vec![QueryValue::Int(i), QueryValue::Int(i)]),
        );
        b.insert(format!("i{i}"), insert("items").row(row));
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
        "F-1 (#792): List is a container TypeTag, excluded from the schema-typed-scalar gate \
         regardless of schema -- this column must fall back to PaginationMode::Offset from \
         creation"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once via offset-mode pagination -- no silent drop"
    );
}

/// Regression guard: a `Str` ORDER BY column (one of the types that was
/// ALREADY safe, per CR-D2's own regression-test convention) must stay green
/// AND must still actually exercise the real Keyset code path -- this fix
/// must not accidentally start routing safe types through the new fallback
/// check.
///
/// F-1 (#792): `String` is one of the accepted `TypeTag`s in the
/// schema-typed-scalar gate, so this test binds a schema declaring
/// `score: String, required` -- without it, this schemaless column would now
/// (correctly) fall back to `PaginationMode::Offset` from creation, same as
/// every other schemaless column, and this test would no longer exercise
/// genuine Keyset-mode seeking at all.
#[tokio::test]
async fn str_order_by_regression_still_uses_real_keyset_seek() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");
    bind_schema(
        &shamir,
        "app",
        "main",
        "items",
        "score",
        shamir_db::engine::validator::schema::TypeTag::String,
    )
    .await;

    let mut b = Batch::new();
    let labels = ["ee", "cc", "aa", "dd", "bb"];
    for (i, label) in labels.iter().enumerate() {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64, "score" => label.to_string() }),
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
        PaginationMode::Keyset,
        "a Str ORDER BY column was already safe pre-this-task -- must stay Keyset mode, not \
         get accidentally routed through the new Bin/List/Dec/Big fallback check"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "regression: a Str ORDER BY column must still page correctly via genuine keyset \
         seeking (labels sort aa < bb < cc < dd < ee)"
    );
}

/// Positive keyset test for `Bool` (F-18, #811): both the `/crush` post-wave
/// review and the deeper static-audit review flagged that `Bool` is one of
/// the accepted `TypeTag`s in the schema-typed-scalar gate and is genuinely
/// comparable via `compare_values` (`resolve.rs`'s `(Value::Bool(a),
/// Value::Bool(b)) => Some(a.cmp(b))` arm, `false < true`) -- yet, unlike
/// `Int`/`Str`, there was no POSITIVE test proving a schema-typed `Bool`
/// ORDER BY column actually reaches `PaginationMode::Keyset` and pages
/// correctly through genuine keyset seeking, not just the offset fallback.
#[tokio::test]
async fn bool_order_by_regression_uses_real_keyset_seek() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");
    bind_schema(
        &shamir,
        "app",
        "main",
        "items",
        "score",
        shamir_db::engine::validator::schema::TypeTag::Bool,
    )
    .await;

    let mut b = Batch::new();
    let flags = [true, false, true, false, true];
    for (i, flag) in flags.iter().enumerate() {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64, "score" => *flag }),
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
        PaginationMode::Keyset,
        "a schema-typed Bool ORDER BY column is one of the accepted TypeTags in the \
         schema-typed-scalar gate, and compare_values has a working (Bool, Bool) arm -- this \
         must reach genuine PaginationMode::Keyset, not fall back to Offset"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once via genuine keyset seeking over a Bool ORDER BY \
         column (false < true, with tie runs on each side)"
    );
}

/// Regression guard: an `Int` ORDER BY column with duplicate-value tie runs
/// (the review's own CR-A4 scenario) must remain unaffected by this task's
/// new per-value safety check -- `Int` is trivially convertible AND
/// comparable, so `safe_seek_key` must be a no-op for it.
#[tokio::test]
async fn int_order_by_regression_still_uses_real_keyset_seek() {
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
        "Int must stay Keyset mode -- unaffected by the new Bin/List/Dec/Big safety check"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3],
        "regression: Int tie-run handling (CR-A4) must remain intact"
    );
}

/// Regression guard: an `F64` ORDER BY column (no NaN, no ties) must still
/// paginate correctly -- but, since F-1 (#792), via `PaginationMode::Offset`,
/// not genuine keyset seeking. `F64` is UNCONDITIONALLY excluded from the
/// schema-typed-scalar gate's accepted `TypeTag` set (schema enforcement
/// cannot rule out `NaN` on an `F64` field, per `FieldRule::check_f64`), so
/// this column can never reach Keyset mode regardless of whether a schema is
/// bound -- distinct from the `int_order_by_regression` test, which DOES
/// stay Keyset for the OTHER previously-safe numeric scalar type once a
/// schema is bound.
#[tokio::test]
async fn f64_order_by_regression_paginates_correctly_via_offset() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    let mut b = Batch::new();
    let values = [5.5f64, 3.3, 1.1, 4.4, 2.2];
    for (i, v) in values.iter().enumerate() {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i as i64, "score" => *v }),
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
        "F-1 (#792): F64 must NEVER reach Keyset mode -- the schema-typed-scalar gate \
         unconditionally excludes F64 (NaN cannot be ruled out even under a schema)"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "regression: a non-NaN F64 ORDER BY column must still page correctly, now via \
         offset-mode pagination"
    );
}

// ---------------------------------------------------------------------------
// F-1 (#792, release blocker) — keyset cursor: gate on schema-typed
// homogeneous scalar.
//
// Two residual gaps left open by CR-D2/W-2/W-3 (mixed `QueryValue` type in
// one ORDER BY column; `NaN` in an `F64` column) have no cheap runtime probe
// the way `Null` has `Filter::IsNull`. Instead of a probe, `create_cursor`
// now gates keyset eligibility on the table's bound SCHEMA (if any): a
// schema-enforced, fixed, non-container scalar `TypeTag` (`Int`/`Bool`/
// `String`) on the ORDER BY column proves every row satisfying that
// schema is provably homogeneous by construction (mixed-type is
// impossible). `F64`/`Dec`/`Big`/containers/`Any` stay excluded even under
// schema (schema enforcement does not reject `NaN` on `F64`, and containers
// can hold heterogeneous nested values). `Bin` was originally accepted here
// too but was excluded by F-18 (#811): `compare_values` never had a
// `(Bin, Bin)` arm, so a `Bin` column could never actually benefit from
// Keyset mode. A column with NO bound schema at all (the common
// schemaless-document case) keeps using `PaginationMode::Offset`
// unconditionally, conservative by default.
// ---------------------------------------------------------------------------

/// A table with a schema declaring the ORDER BY field as `Int` and
/// `required: true` -> cursor uses `PaginationMode::Keyset`. This is the
/// core positive case the schema gate exists to enable.
#[tokio::test]
async fn schema_typed_int_required_order_by_uses_keyset_mode() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");
    bind_schema(
        &shamir,
        "app",
        "main",
        "items",
        "score",
        shamir_db::engine::validator::schema::TypeTag::Int,
    )
    .await;

    let mut b = Batch::new();
    for i in 0..5i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => 4 - i }),
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
        PaginationMode::Keyset,
        "a schema-typed Int (required) ORDER BY column passes the schema gate and reaches Keyset \
         mode -- bind_schema sets keyset_safe: true (table was empty at bind time)"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once"
    );
}

/// A table with a schema declaring the ORDER BY field as `F64` -> cursor
/// falls back to `Offset` even though the shape check alone would have
/// picked `Keyset` -- regression test proving the `F64` exclusion holds even
/// when a schema IS bound (schema enforcement cannot rule out `NaN` on an
/// `F64` field, per `FieldRule::check_f64`, so `F64` is excluded
/// unconditionally, not just for schemaless columns).
#[tokio::test]
async fn schema_typed_f64_order_by_still_forces_offset_mode() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");
    bind_schema(
        &shamir,
        "app",
        "main",
        "items",
        "score",
        shamir_db::engine::validator::schema::TypeTag::F64,
    )
    .await;

    let mut b = Batch::new();
    for i in 0..5i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => (4 - i) as f64 }),
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
        "F64 must be excluded from the schema-typed-scalar gate even when a schema IS \
         bound -- schema enforcement cannot rule out NaN on an F64 field"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once"
    );
}

/// A table with NO schema bound at all (schemaless) -> cursor falls back to
/// `Offset` -- regression test for the conservative default, using a plain
/// `Int` ORDER BY column that WOULD have been keyset-eligible under the OLD
/// shape-only gate.
#[tokio::test]
async fn schemaless_table_order_by_forces_offset_mode() {
    // build_handler_with_rows seeds a schemaless `app.main.items` table with
    // no validator bound at all.
    let handler = build_handler_with_rows(5, CursorLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("v"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seen_vs) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            page,
            has_more,
        } => {
            assert!(has_more, "5 rows / page_size 2 -> more pages remain");
            let vs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("v").expect("v present"))
                .collect();
            (cursor_id, vs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "F-1 (#792): a table with NO schema validator bound at all must fall back to \
         PaginationMode::Offset unconditionally, even for an otherwise keyset-shape-eligible \
         Int ORDER BY column -- the conservative default for the common schemaless-document \
         case"
    );

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
        (0..5).collect::<Vec<i64>>(),
        "every row must appear exactly once under offset-mode pagination"
    );
}
