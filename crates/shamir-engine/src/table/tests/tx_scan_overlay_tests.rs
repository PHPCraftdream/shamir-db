//! FG-3: match-scan overlay-awareness in `execute_update_tx` /
//! `execute_delete_tx` (the #729-residual gap), plus the unique-validator
//! cross-row staged-visibility mandatory test.
//!
//! Mandatory tests 4, 5, 6 of the FG-3 brief:
//! 4. UPDATE matches a staged insert — BOTH the index-path and
//!    list_stream-fallback arms of `execute_update_tx`'s match-scan.
//! 5. DELETE matches a staged insert — symmetric.
//! 6. Unique-constraint validator sees staged rows within the same tx
//!    (already covered by pre-existing `ValidatorDb::exists_in_self` RYOW
//!    logic — this test proves it, it required no new fix).

use std::sync::Arc;

use shamir_query_builder::{filter, write};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::validator::schema::{Constraints, FieldRule, SchemaValidator, TypeTag};
use crate::validator::{RecordValidator, ValidatorBinding, ValidatorRegistry, WriteOp};

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

fn row(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    QueryValue::Map(m)
}

// ============================================================================
// Test 4 — UPDATE matches a staged insert (list_stream-fallback arm, no index)
// ============================================================================

/// In one tx: insert a row (staged, not committed), then run an
/// `UPDATE ... WHERE` matching a NON-indexed field on that just-inserted
/// row. Before the FG-3 fix, `execute_update_tx`'s match-scan was
/// committed-store-only and would find nothing (affected: 0). After the
/// fix, `staged_only_matches` folds the staged row into `matched`.
#[tokio::test]
async fn update_matches_staged_insert_fallback_arm() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    // Mark implicit so new field names intern straight into the BASE
    // interner (matches production's `run_implicit_batch_tx` convention) —
    // otherwise a field name only ever staged into `tx.interner_overlay`
    // would not resolve when `compile_filter` builds the WHERE-clause
    // filter against the base interner, an orthogonal pre-existing
    // constraint unrelated to the FG-3 overlay-merge fix under test here.
    tx.set_implicit(true);

    // Insert row A — staged, uncommitted.
    let insert_op = write::insert("t")
        .row(row(vec![
            ("name", QueryValue::Str("Alice".into())),
            ("status", QueryValue::Str("pending".into())),
        ]))
        .build();
    table
        .execute_insert_tx(
            &insert_op,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    // UPDATE ... WHERE name == "Alice" — no index on "name", so this is the
    // list_stream-fallback arm. Must match the staged-only row in the SAME tx.
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let update_op = write::update("t")
        .where_(filter::eq("name", "Alice"))
        .set(row(vec![("status", QueryValue::Str("active".into()))]))
        .build();
    let result = table
        .execute_update_tx(
            &update_op,
            &ctx,
            &mut tx,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    assert_eq!(
        result.affected, 1,
        "UPDATE must match the row this SAME tx just staged-inserted (fallback arm)"
    );

    repo.commit_tx(tx).await.unwrap();
}

/// Same as above but exercising the INDEX-PATH arm: a regular index exists
/// on "status" and the UPDATE's WHERE is a simple Eq on that indexed field.
/// The index itself never contains the staged-only row (indexing happens
/// at commit/stage-apply time) — `execute_update_tx` must still fold it in
/// via `staged_only_matches` after the index lookup returns.
#[tokio::test]
async fn update_matches_staged_insert_index_arm() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();

    // Pre-populate one committed row so the index isn't trivially empty,
    // then build the index.
    let interner = table.interner().get().await.unwrap();
    let seed = row(vec![
        ("name", QueryValue::Str("Seed".into())),
        ("status", QueryValue::Str("other".into())),
    ]);
    let (inner_val, new_keys) =
        crate::table::tests::test_helpers::query_value_to_inner_tracked(&seed, interner).unwrap();
    if !new_keys.is_empty() {
        table.interner().save_new_keys(&new_keys).await.unwrap();
    }
    table.insert(&inner_val).await.unwrap();
    table.create_index("status_idx", &["status"]).await.unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    // Mark implicit so new field names intern straight into the BASE
    // interner (matches production's `run_implicit_batch_tx` convention) —
    // otherwise a field name only ever staged into `tx.interner_overlay`
    // would not resolve when `compile_filter` builds the WHERE-clause
    // filter against the base interner, an orthogonal pre-existing
    // constraint unrelated to the FG-3 overlay-merge fix under test here.
    tx.set_implicit(true);

    // Insert row A with status="pending" — staged, uncommitted. Never
    // reaches the index (indexing happens at commit).
    let insert_op = write::insert("t")
        .row(row(vec![
            ("name", QueryValue::Str("Alice".into())),
            ("status", QueryValue::Str("pending".into())),
        ]))
        .build();
    table
        .execute_insert_tx(
            &insert_op,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    // UPDATE ... WHERE status == "pending" — a simple Eq on the INDEXED
    // field, so `lookup_records_via_index` returns Some(...) (the index
    // arm). The index itself has no entry for the staged row (never
    // committed), so the index lookup alone would return empty — the
    // staged-only fold-in must still find and match it.
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let update_op = write::update("t")
        .where_(filter::eq("status", "pending"))
        .set(row(vec![("status", QueryValue::Str("active".into()))]))
        .build();
    let result = table
        .execute_update_tx(
            &update_op,
            &ctx,
            &mut tx,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    assert_eq!(
        result.affected, 1,
        "UPDATE must match the staged-inserted row via the INDEX arm's staged-only fold-in \
         (the index itself never sees a staged-only row)"
    );

    repo.commit_tx(tx).await.unwrap();
}

// ============================================================================
// Test 5 — DELETE matches a staged insert (symmetric to test 4)
// ============================================================================

#[tokio::test]
async fn delete_matches_staged_insert_fallback_arm() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    // Mark implicit so new field names intern straight into the BASE
    // interner (matches production's `run_implicit_batch_tx` convention) —
    // otherwise a field name only ever staged into `tx.interner_overlay`
    // would not resolve when `compile_filter` builds the WHERE-clause
    // filter against the base interner, an orthogonal pre-existing
    // constraint unrelated to the FG-3 overlay-merge fix under test here.
    tx.set_implicit(true);

    let insert_op = write::insert("t")
        .row(row(vec![("name", QueryValue::Str("Alice".into()))]))
        .build();
    table
        .execute_insert_tx(
            &insert_op,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let delete_op = write::delete("t")
        .where_(filter::eq("name", "Alice"))
        .build();
    let result = table
        .execute_delete_tx(
            &delete_op,
            &ctx,
            &mut tx,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    assert_eq!(
        result.affected, 1,
        "DELETE must match the row this SAME tx just staged-inserted (fallback arm)"
    );

    repo.commit_tx(tx).await.unwrap();
}

#[tokio::test]
async fn delete_matches_staged_insert_index_arm() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();

    let interner = table.interner().get().await.unwrap();
    let seed = row(vec![
        ("name", QueryValue::Str("Seed".into())),
        ("status", QueryValue::Str("other".into())),
    ]);
    let (inner_val, new_keys) =
        crate::table::tests::test_helpers::query_value_to_inner_tracked(&seed, interner).unwrap();
    if !new_keys.is_empty() {
        table.interner().save_new_keys(&new_keys).await.unwrap();
    }
    table.insert(&inner_val).await.unwrap();
    table.create_index("status_idx", &["status"]).await.unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    // Mark implicit so new field names intern straight into the BASE
    // interner (matches production's `run_implicit_batch_tx` convention) —
    // otherwise a field name only ever staged into `tx.interner_overlay`
    // would not resolve when `compile_filter` builds the WHERE-clause
    // filter against the base interner, an orthogonal pre-existing
    // constraint unrelated to the FG-3 overlay-merge fix under test here.
    tx.set_implicit(true);

    let insert_op = write::insert("t")
        .row(row(vec![
            ("name", QueryValue::Str("Alice".into())),
            ("status", QueryValue::Str("pending".into())),
        ]))
        .build();
    table
        .execute_insert_tx(
            &insert_op,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let delete_op = write::delete("t")
        .where_(filter::eq("status", "pending"))
        .build();
    let result = table
        .execute_delete_tx(
            &delete_op,
            &ctx,
            &mut tx,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    assert_eq!(
        result.affected, 1,
        "DELETE must match the staged-inserted row via the INDEX arm's staged-only fold-in"
    );

    repo.commit_tx(tx).await.unwrap();
}

// ============================================================================
// Test 6 — Unique-constraint validator sees staged rows within one tx
// ============================================================================

/// A `unique` validator bound to `t.email`. Within ONE tx: insert row A
/// (email "x@example.com"), then attempt to insert row B ALSO with
/// "x@example.com" in the SAME tx (before commit). The validator must
/// reject B, proving cross-row uniqueness checks see the tx's own staged
/// data, not just the committed snapshot.
///
/// This exercises `ValidatorDb::exists_in_self`'s pre-existing staged-probe
/// (step 3 in `validator_db.rs`, added by an earlier task, RI-7/C3) — NOT
/// `list_stream_tx`/`filter_stream_tx` (which `run_validators_qv` never
/// routes through). No new fix was required for this test to pass; it
/// documents that item 6 of the FG-3 brief was already covered.
#[tokio::test]
async fn unique_validator_sees_staged_row_in_same_tx() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let mut table = repo.get_table("t").await.unwrap();

    let rule = FieldRule {
        path: vec!["email".to_string()],
        ty: TypeTag::String,
        constraints: Constraints {
            unique: true,
            ..Default::default()
        },
    };
    let validator = Arc::new(SchemaValidator::new(vec![rule])) as Arc<dyn RecordValidator>;
    let reg = Arc::new(ValidatorRegistry::new());
    let validator_id = shamir_types::types::record_id::RecordId::system("unique_email");
    reg.register(validator_id, "unique_email", validator)
        .unwrap();
    table.set_validator_registry(reg);
    table
        .add_validator_binding(ValidatorBinding {
            validator_id,
            ops: smallvec::smallvec![WriteOp::Insert],
            priority: 1000,
        })
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    // Mark implicit so new field names intern straight into the BASE
    // interner (matches production's `run_implicit_batch_tx` convention) —
    // otherwise a field name only ever staged into `tx.interner_overlay`
    // would not resolve when `compile_filter` builds the WHERE-clause
    // filter against the base interner, an orthogonal pre-existing
    // constraint unrelated to the FG-3 overlay-merge fix under test here.
    tx.set_implicit(true);

    // Row A — accepted (no prior committed or staged duplicate).
    let insert_a = write::insert("t")
        .row(row(vec![(
            "email",
            QueryValue::Str("x@example.com".into()),
        )]))
        .build();
    let res_a = table
        .execute_insert_tx(
            &insert_a,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await;
    assert!(res_a.is_ok(), "row A must be accepted: {res_a:?}");

    // Row B — SAME tx, SAME email, NOT yet committed. Must be rejected
    // because `exists_in_self` probes `tx.write_set` (row A's staged Set)
    // in addition to the committed snapshot.
    let insert_b = write::insert("t")
        .row(row(vec![(
            "email",
            QueryValue::Str("x@example.com".into()),
        )]))
        .build();
    let res_b = table
        .execute_insert_tx(
            &insert_b,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await;
    assert!(
        res_b.is_err(),
        "row B (in-tx duplicate of row A's staged email) must be REJECTED by the unique validator, got: Ok"
    );
}
