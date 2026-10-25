//! Exact numeric FK matching (RV-FUNCLIB-5FA455F7, S2): FK matching must
//! use `scalar_ref_numeric_eq` (exact Int↔F64/Big, NaN matches nothing)
//! instead of the lossy `scalar_ref_cmp_qv == Equal` path that collapsed
//! distinct integers beyond `2^53` through `a as f64`.
//!
//! Scope note: the batch FK harness only binds Delete/Update ops (the
//! forward insert-side gate needs a resolver wired into `ValidatorDb`,
//! which the implicit write path does not provide — see
//! `fk_restrict_tests.rs`'s setup comment). The INSERT-side matching
//! semantics are therefore covered by calling `fk_restrict`'s matching
//! function (`scalar_ref_matches_qv` — the funnel every insert/delete/
//! update path goes through) DIRECTLY; the DELETE side gets an end-to-end
//! restrict test.

use num_bigint::BigInt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::query::batch::fk::fk_restrict::scalar_ref_matches_qv;
use crate::query::batch::tests::fk::actions::support::bind_fk_validator;
use crate::query::batch::tests::fk::support::FkTestResolver;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::validator::validator_db::scalar_ref_matches_query_value;
use crate::validator::ValidatorRegistry;

// ── Direct funnel tests (insert/delete/update all go through this fn) ───────

/// A parent key `Int(2^53+1)` matches a child FK value of exactly
/// `Big(2^53+1)` but REJECTS `Big(2^53+2)` — the old lossy path matched
/// BOTH (both round-trip through `f64(2^53)`).
#[test]
fn rv_funclib_5fa455f7_fk_match_int_big_exact_beyond_2p53() {
    let parent = 9007199254740993i64; // 2^53 + 1
    assert!(scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::Int(parent),
        &QueryValue::Big(BigInt::from(parent))
    ));
    // The case ONLY the predicate wiring fixes: Int(2^53+1) vs Big(2^53)
    // WAS `Equal` on the old ordering path (`(a as f64).partial_cmp(&
    // big_to_f64(b))` — both sides collapse to f64(2^53)); it must be
    // false now.
    assert!(!scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::Int(parent),
        &QueryValue::Big(BigInt::from(parent - 1))
    ));
    assert!(!scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::Int(parent),
        &QueryValue::Big(BigInt::from(parent + 1))
    ));
    // Sanity: same-value Int/F64 match, different values don't.
    assert!(scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::F64(1.0),
        &QueryValue::Int(1)
    ));
    assert!(!scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::F64(1.0),
        &QueryValue::Int(2)
    ));
}

/// NaN never FK-matches — not an Int parent, not even another NaN.
#[test]
fn rv_funclib_5fa455f7_fk_match_nan_never_matches() {
    assert!(!scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::F64(f64::NAN),
        &QueryValue::Int(1)
    ));
    assert!(!scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::Int(1),
        &QueryValue::F64(f64::NAN)
    ));
    assert!(!scalar_ref_matches_qv(
        &shamir_types::record_view::ScalarRef::F64(f64::NAN),
        &QueryValue::F64(f64::NAN)
    ));
}

/// INSERT-side cross-type matching: `ValidatorDb`'s equality funnel
/// (`scalar_ref_matches_query_value`, the fn behind `exists_in`'s scan /
/// staged-probe / index-recheck paths) must use the ONE-rule predicate —
/// an `Int(1)` field matches an `F64(1.0)` probe, `Int(2^53+1)` does NOT
/// match an `F64(2^53)` probe (the old variant-strict matcher rejected the
/// first pair, a lossy matcher would wrongly accept the second), and NaN
/// matches nothing. Fails while the variant-strict matcher is in place.
#[test]
fn rv_funclib_5fa455f7_fk_insert_side_validator_cross_type() {
    assert!(scalar_ref_matches_query_value(
        &ScalarRef::Int(1),
        &QueryValue::F64(1.0)
    ));
    assert!(scalar_ref_matches_query_value(
        &ScalarRef::F64(1.0),
        &QueryValue::Int(1)
    ));
    assert!(!scalar_ref_matches_query_value(
        &ScalarRef::Int(9_007_199_254_740_993), // 2^53 + 1
        &QueryValue::F64(9_007_199_254_740_992.0)
    ));
    assert!(!scalar_ref_matches_query_value(
        &ScalarRef::F64(f64::NAN),
        &QueryValue::F64(f64::NAN)
    ));
    assert!(!scalar_ref_matches_query_value(
        &ScalarRef::Int(1),
        &QueryValue::F64(f64::NAN)
    ));
}

// ── End-to-end DELETE-side restrict ─────────────────────────────────────────

/// Deleting a parent is RESTRICTED while a child row holds an exactly-equal
/// key. Case 1 (revert-sensitive): parent `Int(1)`, child `F64(1.0)` — the
/// stored F64 round-trips the wire, so only the ONE-rule FK predicate wires
/// this restrict decision (the old variant-strict matcher would let the
/// delete succeed). Case 2: parent `Int(2^53+1)`, child `Int(2^53+1)` —
/// exact beyond `2^53`; the `Big` cross-type behaviour is pinned by the
/// direct funnel tests above.
#[tokio::test]
async fn rv_funclib_5fa455f7_fk_restrict_parent_delete_with_exact_child() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = std::sync::Arc::new(ValidatorRegistry::new());
    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_schema",
        "parent_id",
        "parent",
        "id",
        FkAction::Restrict,
        false,
    );
    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    let parent_key = 9007199254740993i64; // 2^53 + 1

    // ── Case 1 (revert-sensitive): parent Int(1), child F64(1.0) ────────
    // The stored F64 round-trips the wire, so ONLY the FK predicate wiring
    // (numeric key equality) makes the restrict decision: the old
    // variant-strict matcher would NOT match Int(1) vs F64(1.0) → the
    // delete would succeed → this test fails on the revert.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent1",
        write::insert("parent").row(
            doc()
                .set_value("id", QueryValue::Int(1))
                .set("name", "P1")
                .unwrap(),
        ),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child1",
        write::insert("child").row(
            doc()
                .set_value("parent_id", QueryValue::F64(1.0))
                .set("label", "x")
                .unwrap(),
        ),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let mut b = Batch::new();
    b.id(3);
    b.try_delete(
        "del_parent1",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;
    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "error should contain 'fk_restrict', got: {msg}"
            );
        }
        Ok(r) => panic!(
            "Expected fk_restrict error, got success: {:?}",
            r.results["del_parent1"]
        ),
    }

    // ── Case 2: parent Int(2^53+1), child Int(2^53+1) — exact beyond 2^53.
    // Insert the parent row.
    let mut b = Batch::new();
    b.id(4);
    b.insert(
        "ins_parent",
        write::insert("parent").row(
            doc()
                .set_value("id", QueryValue::Int(parent_key))
                .set("name", "P")
                .unwrap(),
        ),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Insert a child referencing it.
    let mut b = Batch::new();
    b.id(5);
    b.insert(
        "ins_child",
        write::insert("child").row(
            doc()
                .set_value("parent_id", QueryValue::Int(parent_key))
                .set("label", "x")
                .unwrap(),
        ),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["ins_child"].records.len(), 1);

    // Delete the parent → restricted.
    let mut b = Batch::new();
    b.id(6);
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", parent_key)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;
    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "error should contain 'fk_restrict', got: {msg}"
            );
        }
        Ok(r) => panic!(
            "Expected fk_restrict error, got success: {:?}",
            r.results["del_parent"]
        ),
    }
}
