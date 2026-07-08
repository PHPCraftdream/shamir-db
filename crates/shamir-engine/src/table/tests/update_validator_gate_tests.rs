//! Correctness regression tests for the `has_update_validators` /
//! `has_upsert_validators` hoist in `execute_update_tx` /
//! `execute_set_tx` (audit `2026-07-06-perf-hot-paths.md` §1.3).
//!
//! The optimisation gates the per-row `old_qv` / `new_qv` de-intern +
//! `run_validators_qv` call behind a pre-loop check that mirrors
//! `execute_delete_tx`'s `has_delete_validators`. These tests prove the
//! gate does NOT silently break validator enforcement on the optimised
//! path, nor change the observable RETURNING shape on either path.
//!
//! Three guarantees:
//!
//! 1. **UPDATE with a bound Update validator still runs it** — a
//!    rejecting validator must cause `execute_update_tx` to fail (proves
//!    the gate does not skip a real validator).
//! 2. **UPDATE ... RETURNING without Update validators still returns
//!    the correct post-update field values** — proves the
//!    single-de-intern result path (no validator-built `old_qv` to
//!    reuse) produces the same result as before.
//! 3. **SET (upsert) MERGE with a bound Upsert validator still runs
//!    it** — a rejecting validator must cause `execute_set_tx` to fail
//!    on the merge branch (proves the analogous gate on the upsert
//!    path does not skip a real validator). The insert branch is
//!    already covered by `s_write_server_tests`.

use std::sync::Arc;

use async_trait::async_trait;
use smallvec::smallvec;

use shamir_query_builder::filter;
use shamir_query_builder::write::{self, doc, UpdateReturnMode};
use shamir_types::record_view::ScalarRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::table::tests::write_exec_tests::{
    insert_via_tx, set_via_tx, setup_empty_table, update_via_tx,
};
use crate::validator::{
    RecordFields, RecordValidator, Validation, ValidatorBinding, ValidatorCtx, ValidatorRegistry,
    WriteOp,
};
use shamir_types::types::common::new_map;

// ============================================================================
// Stub validator
// ============================================================================

/// Rejects any record whose `status` field is `"banned"`. Used for both
/// the Update-path and Upsert-merge-path tests.
struct RejectBannedStatus;

#[async_trait]
impl RecordValidator for RejectBannedStatus {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        let banned = new
            .and_then(|f| f.scalar(&["status"]))
            .map(|s| matches!(s, ScalarRef::Str(s) if s == "banned"));
        if banned.unwrap_or(false) {
            Validation::reject("status_banned")
        } else {
            Validation::accept()
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Wire a `RejectBannedStatus` validator into `table` bound to `op`.
/// Returns nothing; the validator is registered + bound in-place.
async fn bind_banned_validator(table: &mut crate::table::TableManager, op: WriteOp) {
    let val_id = RecordId::system("reject_banned_status");
    let reg = Arc::new(ValidatorRegistry::new());
    reg.register(
        val_id,
        "reject_banned_status",
        Arc::new(RejectBannedStatus) as Arc<dyn RecordValidator>,
    )
    .unwrap();
    table.set_validator_registry(reg);
    table
        .add_validator_binding(ValidatorBinding {
            validator_id: val_id,
            ops: smallvec![op],
            priority: 1000,
        })
        .await
        .unwrap();
}

// ============================================================================
// Test 1 — UPDATE with a bound Update validator still runs it
// ============================================================================

/// A registered Update validator must still fire after the
/// `has_update_validators` hoist. Proves the gate does not skip a real
/// validator: an UPDATE setting `status = "banned"` is rejected, while a
/// benign UPDATE setting `status = "premium"` succeeds.
#[tokio::test]
async fn update_validator_runs_when_bound() {
    let (mut table, repo) = setup_empty_table().await;
    bind_banned_validator(&mut table, WriteOp::Update).await;

    // Seed a row.
    let op_ins = write::insert("users")
        .row(doc().set("name", "Alice").set("status", "active"))
        .build();
    insert_via_tx(&repo, &table, &op_ins, false).await.unwrap();

    let refs = new_map();

    // Benign UPDATE — validator accepts.
    let op_ok = write::update("users")
        .where_(filter::eq("name", "Alice"))
        .set(doc().set("status", "premium"))
        .build();
    let res_ok = update_via_tx(&repo, &table, &op_ok, &refs).await;
    assert!(
        res_ok.is_ok(),
        "benign UPDATE must be accepted by the validator, got: {res_ok:?}"
    );
    assert_eq!(res_ok.unwrap().affected, 1);

    // Rejecting UPDATE — validator must block it.
    let op_bad = write::update("users")
        .where_(filter::eq("name", "Alice"))
        .set(doc().set("status", "banned"))
        .build();
    let res_bad = update_via_tx(&repo, &table, &op_bad, &refs).await;
    assert!(
        res_bad.is_err(),
        "UPDATE setting status=banned must be rejected by the bound validator, got: Ok"
    );
}

// ============================================================================
// Test 2 — UPDATE ... RETURNING without Update validators still correct
// ============================================================================

/// With NO Update validators bound, the `has_update_validators` gate is
/// false, so `old_qv` is never built for the validator call. RETURNING
/// must then build exactly ONE de-intern (the result path) and still
/// produce the correct post-update field values. This is the
/// correctness proof for the optimised no-validator RETURNING path.
#[tokio::test]
async fn update_returning_correct_without_validators() {
    let (table, repo) = setup_empty_table().await;

    // Seed two rows.
    let op_ins = write::insert("users")
        .row(doc().set("name", "Alice").set("age", 30_i64))
        .row(doc().set("name", "Bob").set("age", 25_i64))
        .build();
    insert_via_tx(&repo, &table, &op_ins, false).await.unwrap();

    let refs = new_map();

    // Mass UPDATE with RETURNING All — no validators anywhere.
    let op = write::update("users")
        .set(doc().set("status", "premium"))
        .returning(UpdateReturnMode::All)
        .build();
    let result = update_via_tx(&repo, &table, &op, &refs).await.unwrap();

    assert_eq!(result.affected, 2);
    assert_eq!(
        result.records.len(),
        2,
        "RETURNING All must return both rows"
    );

    // Every returned row must carry the post-update `status` overlay AND
    // the pre-existing `name`/`age` fields (proves the single de-intern
    // from old_bytes + overlay produced the full correct row).
    let mut saw_alice = false;
    let mut saw_bob = false;
    for rec in &result.records {
        assert_eq!(
            rec.get_value_owned("status"),
            Some(QueryValue::Str("premium".into())),
            "post-update status overlay missing on a RETURNING row"
        );
        match rec
            .get_value_owned("name")
            .as_ref()
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
        {
            Some(name) if name == "Alice" => {
                saw_alice = true;
                assert_eq!(
                    rec.get_value_owned("age"),
                    Some(QueryValue::Int(30)),
                    "pre-existing age field lost on RETURNING row"
                );
            }
            Some(name) if name == "Bob" => {
                saw_bob = true;
                assert_eq!(
                    rec.get_value_owned("age"),
                    Some(QueryValue::Int(25)),
                    "pre-existing age field lost on RETURNING row"
                );
            }
            other => panic!("unexpected name on RETURNING row: {other:?}"),
        }
    }
    assert!(saw_alice && saw_bob, "RETURNING rows missing Alice/Bob");
}

// ============================================================================
// Test 3 — SET (upsert) MERGE with a bound Upsert validator still runs it
// ============================================================================

/// A registered Upsert validator must still fire on the MERGE branch of
/// `execute_set_tx` after the `has_upsert_validators` hoist. The first
/// SET on a fresh key hits the INSERT branch; the second SET on the SAME
/// key hits the MERGE branch — that is the path the gate optimises, and
/// the rejecting validator must block it.
#[tokio::test]
async fn set_merge_validator_runs_when_bound() {
    let (mut table, repo) = setup_empty_table().await;
    bind_banned_validator(&mut table, WriteOp::Upsert).await;

    // First SET on key {name: "Alice"} → INSERT branch (validator accepts
    // status="active"). Establishes the row so the next SET merges.
    let op_ins = write::upsert("users")
        .key(mpack_key("name", "Alice"))
        .value(doc().set("name", "Alice").set("status", "active"))
        .build();
    let res_ins = set_via_tx(&repo, &table, &op_ins).await;
    assert!(
        res_ins.is_ok(),
        "initial SET insert must succeed: {res_ins:?}"
    );
    assert!(res_ins.unwrap().records[0]
        .get_value_owned("_created")
        .map(|v| matches!(v, QueryValue::Bool(b) if b))
        .unwrap_or(false));

    // Second SET on the same key → MERGE branch. status="banned" must be
    // rejected by the bound Upsert validator.
    let op_merge_bad = write::upsert("users")
        .key(mpack_key("name", "Alice"))
        .value(doc().set("name", "Alice").set("status", "banned"))
        .build();
    let res_merge_bad = set_via_tx(&repo, &table, &op_merge_bad).await;
    assert!(
        res_merge_bad.is_err(),
        "SET MERGE setting status=banned must be rejected by the bound Upsert validator, got: Ok"
    );

    // Sanity: a benign MERGE on the same key still succeeds (validator
    // accepts status="premium") and reports _created=false (merge, not insert).
    let op_merge_ok = write::upsert("users")
        .key(mpack_key("name", "Alice"))
        .value(doc().set("name", "Alice").set("status", "premium"))
        .build();
    let res_merge_ok = set_via_tx(&repo, &table, &op_merge_ok).await;
    assert!(
        res_merge_ok.is_ok(),
        "benign SET MERGE must be accepted, got: {res_merge_ok:?}"
    );
    let rec = &res_merge_ok.unwrap().records[0];
    assert_eq!(
        rec.get_value_owned("status"),
        Some(QueryValue::Str("premium".into())),
        "SET MERGE result must carry the overlaid status"
    );
    assert_eq!(
        rec.get_value_owned("_created"),
        Some(QueryValue::Bool(false)),
        "second SET on same key must report _created=false (merge branch)"
    );
}

/// Build a single-field `{ <key>: <value> }` QueryValue map for the SET
/// `key` argument (matches how the upsert builder expects a key document).
fn mpack_key(key: &str, val: &str) -> QueryValue {
    let mut m = new_map();
    m.insert(key.to_string(), QueryValue::Str(val.into()));
    QueryValue::Map(m)
}
