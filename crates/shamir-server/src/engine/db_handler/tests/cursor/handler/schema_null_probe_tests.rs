//! Schema-typed nullable columns composing with the null probe, mixed-type
//! ORDER BY columns, schema binding after mixed data, DDL on empty tables
//! and upsert-idempotence proofs.

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
use shamir_query_builder::ddl::{add_schema_rule, field, set_table_schema};
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use ;
use ;
use shamir_query_types::filter::FilterValue;
use ;
use ;
use shamir_query_types::read::{
    OrderByReadQuery,
};
use shamir_query_types::wire::{DbResponse};
use ;

use crate::cursor_registry::PaginationMode;
use crate::db_handler::config::CursorLimitsCap;
use crate::db_handler::handler::ShamirDbHandler;
use ;

use super::common::*;

/// A table with a schema declaring the field as `Int` but the field is
/// `nullable`/optional (not `required`) -> schema gate still passes (keyset
/// attempted), but the EXISTING null probe still catches an actual null row
/// and falls back correctly -- proves the two mechanisms compose: the schema
/// gate only proves "no SECOND non-null type is possible", not "never null"
/// (that remains the null probe's job, unchanged by this task).
#[tokio::test]
async fn schema_typed_nullable_int_composes_with_existing_null_probe() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // Nullable (NOT required) Int schema on `score`.
    {
        use shamir_db::engine::validator::schema::{
            Constraints, FieldRule, SchemaValidator, TypeTag,
        };
        use shamir_db::engine::validator::WriteOp;
        use shamir_types::types::record_id::RecordId;

        let constraints = Constraints {
            nullable: true,
            ..Constraints::default()
        };
        let rule = FieldRule {
            path: vec!["score".to_string()],
            ty: TypeTag::Int,
            constraints,
            // F-17 (#810): schema bound BEFORE rows are seeded.
            keyset_safe: true,
        };
        let schema = SchemaValidator::new(vec![rule]);
        let id = RecordId::new();
        shamir
            .validators()
            .register(id, "items_score_nullable_schema", Arc::new(schema))
            .expect("register schema validator");
        shamir
            .bind_validator(
                "app",
                "main",
                "items",
                "items_score_nullable_schema",
                vec![WriteOp::Insert, WriteOp::Update, WriteOp::Upsert],
                1000,
            )
            .await
            .expect("bind schema validator");
    }

    // 3 rows with a real Int score, 2 rows with an explicit Null score.
    let mut b = Batch::new();
    for i in 0..3i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => i }),
        );
    }
    for i in 3..5i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => FilterValue::Null }),
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

    // The schema gate itself passes (Int is accepted, and the gate does not
    // check required/nullable) -- but the pre-existing null probe still
    // detects the actual Null rows in the data and falls the WHOLE cursor
    // back to Offset, exactly as it did before this task for any
    // null-containing column.
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "a nullable (not required) Int-typed column passes the schema gate itself, but the \
         EXISTING null probe must still detect the actual Null rows and fall the whole cursor \
         back to Offset -- the two mechanisms must compose, not conflict"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;

    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row -- including both Null-score rows -- must appear exactly once"
    );
}

/// Core regression this task fixes, end-to-end: a mixed-`QueryValue`-type
/// `ORDER BY` column (some rows `Int`, some rows `Str`) over a SCHEMALESS
/// table. Under the OLD shape-only gate (`pagination_mode_for_query` alone),
/// this cursor would have been pinned to `PaginationMode::Keyset` from
/// creation (a single simple-field ORDER BY is all that check looks at) and
/// would have silently dropped every row of the type NOT sorted onto page 1
/// once the scan passed page 1 (the exact CR-D2 failure shape, but for a
/// value-type mismatch instead of Null). Under the NEW gate, this schemaless
/// column has no bound schema at all, so it is correctly refused keyset
/// eligibility and falls back to `Offset` from creation -- proving no silent
/// row loss occurs for this scenario end-to-end (create cursor, drain every
/// page, assert every row surfaces exactly once).
#[tokio::test]
async fn mixed_type_order_by_column_no_longer_silently_drops_rows() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // 3 rows with an Int score (0, 1, 2), 2 rows with a Str score ("a", "b")
    // -- a mixed-type column, no schema bound at all.
    let mut b = Batch::new();
    for i in 0..3i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => i }),
        );
    }
    let str_scores = ["a", "b"];
    for (j, s) in str_scores.iter().enumerate() {
        let i = 3 + j as i64;
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => s.to_string() }),
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
            cursor_id, page, ..
        } => {
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
        "F-1 (#792): a mixed-type (Int + Str) ORDER BY column with NO schema bound must be \
         refused keyset eligibility from creation -- this is the core regression this task \
         fixes: the OLD shape-only gate would have picked Keyset here and silently dropped \
         rows of whichever type didn't land on page 1"
    );

    // Bounded drain (offset-mode pagination cannot livelock; bounded anyway
    // as cheap insurance against a regression reintroducing one).
    let mut has_more = true;
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
        "every row -- both Int-scored and Str-scored -- must appear exactly once via \
         offset-mode pagination; before this task's fix, this scenario would have silently \
         dropped rows of one type once the keyset scan passed page 1"
    );
}

/// F-17 (#810) core regression: the EXACT scenario from the finding -- a
/// schemaless table with ALREADY-MIXED-TYPE data in the ORDER BY field, THEN
/// a schema rule declaring that field `Int` is bound AFTER the fact via the
/// real DDL path (`set_table_schema`).
///
/// Under the OLD F-1 gate (`order_by_column_is_schema_typed_scalar`), binding
/// an `Int` schema rule would have made the gate return `true` (Keyset
/// enabled) even though the pre-existing `Str` rows silently violate it --
/// `compare_values` has no `(Int, Str)` cross-type arm, so those rows would
/// vanish from later pages with a clean `has_more: false`. F-17's fix stamps
/// `keyset_safe = (table.count() == 0)` at bind time; since the table is
/// non-empty here, `keyset_safe: false` is recorded and the gate refuses
/// keyset eligibility, so this cursor falls back to `PaginationMode::Offset`
/// and every row -- including the pre-schema mixed-type ones -- is returned
/// exactly once.
#[tokio::test]
async fn schema_bound_after_mixed_type_data_still_falls_back_to_offset() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // Step 1-2: insert MIXED-TYPE data into `score` BEFORE any schema is
    // bound -- 3 rows with an Int score, 2 rows with a Str score. This is
    // legal for a schemaless document table.
    let mut b = Batch::new();
    for i in 0..3i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => i }),
        );
    }
    let str_scores = ["a", "b"];
    for (j, s) in str_scores.iter().enumerate() {
        let i = 3 + j as i64;
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => s.to_string() }),
        );
    }
    let batch = b.build();
    shamir
        .execute_as(owner.clone(), "app", &batch)
        .await
        .expect("seed rows");

    // Step 3: NOW bind a schema declaring `score: Int, required` via the
    // real DDL path -- after the mixed-type data already exists. The DDL
    // handler validates the rule's shape but never scans pre-existing rows;
    // `stamp_keyset_safe` computes `table.count() == 0` which is FALSE (5
    // rows exist), so `keyset_safe: false` is recorded.
    let mut b = Batch::new();
    b.set_table_schema(
        "ss",
        set_table_schema("items").rules(vec![field(["score"]).int().required().build()]),
    );
    shamir
        .execute_as(owner, "app", &b.build())
        .await
        .expect("set_table_schema should succeed");

    let handler =
        ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED);
    let session = alice_session();

    // Step 4: open a keyset-requesting cursor ordering by `score`.
    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id, page, ..
        } => {
            let seqs: Vec<i64> = page
                .records
                .iter()
                .map(|r| r.get_value_i64("seq").expect("seq present"))
                .collect();
            (cursor_id, seqs)
        }
        other => panic!("expected CursorPage, got {other:?}"),
    };

    // Step 5: assert Offset mode -- `keyset_safe: false` was stamped because
    // the table was non-empty at bind time, so the gate does NOT trust the
    // post-hoc schema binding as proof of homogeneity for the pre-existing
    // mixed-type rows.
    assert_eq!(
        pinned_mode(&handler, cursor_id, &ALICE_SID),
        PaginationMode::Offset,
        "F-17 (#810): keyset_safe is false (table was non-empty at bind time), so even with an \
         Int schema rule bound AFTER mixed-type data was inserted, the cursor must fall back to \
         PaginationMode::Offset -- the pre-existing Str rows would otherwise be silently dropped \
         by compare_values' boundary filter under Keyset mode"
    );

    // Step 6: drain the full cursor and assert every row -- including the
    // pre-schema mixed-type Str-scored rows -- is returned exactly once.
    let mut has_more = true;
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
        "every row -- including both pre-schema Str-scored rows (seq 3, 4) -- must appear \
         exactly once via offset-mode pagination; under the OLD F-1 gate, the post-hoc schema \
         binding would have enabled Keyset mode and silently dropped the Str rows past page 1"
    );
}

/// F-17 (#810): a schema bound on an EMPTY table via the DDL path must
/// stamp `keyset_safe: true` and the cursor must reach genuine
/// `PaginationMode::Keyset`. This is the "persistence round-trip" proof:
/// the flag is persisted in the catalogue, recompiled into the engine-side
/// `FieldRule`, and consumed by the gate.
#[tokio::test]
async fn ddl_schema_on_empty_table_reaches_keyset_mode() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // Bind schema on an EMPTY table → keyset_safe: true.
    let mut b = Batch::new();
    b.set_table_schema(
        "ss",
        set_table_schema("items").rules(vec![field(["score"]).int().required().build()]),
    );
    shamir
        .execute_as(owner.clone(), "app", &b.build())
        .await
        .expect("set_table_schema should succeed");

    // NOW seed rows (all Int, conforming to the schema).
    let mut b = Batch::new();
    for i in 0..5i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => i * 10 }),
        );
    }
    shamir
        .execute_as(owner, "app", &b.build())
        .await
        .expect("seed rows");

    let handler =
        ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED);
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id, page, ..
        } => {
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
        "F-17 (#810): schema bound on empty table → keyset_safe: true, so the gate passes and \
         the cursor reaches genuine Keyset mode"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;
    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "every row must appear exactly once under Keyset mode"
    );
}

/// F-17 (#810): upsert-by-path with an IDENTICAL rule must preserve the
/// prior `keyset_safe` proof. A rule bound on an empty table (keyset_safe:
/// true) is re-declared via `add_schema_rule` AFTER rows have been seeded
/// (table now non-empty). Because the rule's path AND type are unchanged,
/// `stamp_keyset_safe` preserves the prior proof instead of recomputing
/// (which would incorrectly flip it to false and degrade a working cursor
/// to Offset for no reason).
#[tokio::test]
async fn upsert_identical_rule_preserves_keyset_safe_proof() {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir.create_db_as("app", owner.clone()).await;
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir
        .add_repo_as("app", cfg, owner.clone())
        .await
        .expect("add repo");

    // Step 1: bind schema on an EMPTY table → keyset_safe: true.
    let rule = field(["score"]).int().required().build();
    let mut b = Batch::new();
    b.set_table_schema("ss", set_table_schema("items").rules(vec![rule]));
    shamir
        .execute_as(owner.clone(), "app", &b.build())
        .await
        .expect("set_table_schema should succeed");

    // Step 2: seed rows (table is now non-empty).
    let mut b = Batch::new();
    for i in 0..4i64 {
        b.insert(
            format!("i{i}"),
            insert("items").row(doc! { "seq" => i, "score" => i * 10 }),
        );
    }
    shamir
        .execute_as(owner.clone(), "app", &b.build())
        .await
        .expect("seed rows");

    // Step 3: re-declare the SAME rule via add_schema_rule. The table is
    // non-empty, but since the rule's path + type are UNCHANGED,
    // stamp_keyset_safe preserves the prior keyset_safe: true.
    let mut b = Batch::new();
    b.try_add_schema_rule(
        "ar",
        add_schema_rule("items").rule(field(["score"]).int().required().build()),
    )
    .unwrap();
    shamir
        .execute_as(owner, "app", &b.build())
        .await
        .expect("add_schema_rule should succeed");

    let handler =
        ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED);
    let session = alice_session();

    let query = ReadQuery::new("items").order_by(OrderBy::asc("score"));
    let resp = send(&handler, &session, create_cursor_req(query, 2)).await;
    let (cursor_id, mut seqs) = match resp {
        DbResponse::CursorPage {
            cursor_id, page, ..
        } => {
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
        "F-17 (#810): upsert of an identical rule preserves the prior keyset_safe: true proof \
         (the table was non-empty at re-declaration time, but the rule's path + type are unchanged)"
    );

    drain_seqs(&handler, &session, cursor_id, 2, &mut seqs).await;
    seqs.sort_unstable();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3],
        "every row exactly once under Keyset mode"
    );
}

// ---------------------------------------------------------------------------
// F-53b Step 4 (#880): `PaginationMode::IndexSeek` — production-path tests.
//
// Ports the F-53b Step 3 spike's three tests
// (`crates/shamir-engine/src/table/tests/f53b_step3_cursor_after_spike.rs`)
// to drive the REAL `create_cursor`/`fetch_next` wire handlers, plus a new
// test confirming the fallback holds PERMANENTLY across page 3 (no
// re-probing `IndexSeek`), plus DESC-direction parity. See the settled
// design memo: `docs/dev-artifacts/research/
// f53b-step3-cursor-pagination-after-spike.md`.
