//! Regression test for RV-QUERY-BUILDER-F3ED5A33: `Batch::try_build` must
//! accept a `sub_batch`/`for_each` entry whose INNER batch has internal
//! `$query` dependencies — outer dependency discovery for nested batches is
//! bind-only (`SubBatchOp::bind`) / over-only (`ForEachOp::over`), mirroring
//! the planner (`shamir-query-types` `ops/batch/plan/planner.rs`,
//! `extract_entry_deps`'s `BatchOp::Batch` / `BatchOp::ForEach` arms).

use shamir_collections::new_map;
use shamir_query_types::batch::BatchOp;
use shamir_query_types::filter::FilterValue;

use crate::batch::build_error::BuildError;
use crate::batch::Batch;
use crate::query::Query;
use crate::val::{param, qref};

// rv_query_builder_f3ed5a33_sub_batch_inner_internal_refs_accepted
#[test]
fn rv_query_builder_f3ed5a33_sub_batch_inner_internal_refs_accepted() {
    // Inner batch with an INTERNAL a→b dependency (both aliases live only
    // inside the sub-batch; the outer batch has neither).
    let mut inner = Batch::new();
    inner.query("inner_a", Query::from("products"));
    inner.query(
        "inner_b",
        Query::from("orders").where_eq("pid", qref("inner_a", "id")),
    );

    let mut outer = Batch::new();
    outer.query("seed", Query::from("users"));
    outer.sub_batch("proc", inner.build(), new_map());

    // Old code serialized the whole `BatchOp::Batch` body and validated the
    // inner-internal `@inner_a` ref against the OUTER alias set → false
    // UnknownAlias. The planner binds-only, so try_build must succeed.
    outer
        .try_build()
        .expect("inner-internal $query refs must not be validated against the outer alias set");
}

// rv_query_builder_f3ed5a33_for_each_inner_internal_refs_accepted
#[test]
fn rv_query_builder_f3ed5a33_for_each_inner_internal_refs_accepted() {
    let mut inner = Batch::new();
    inner.query("loop_a", Query::from("products"));
    inner.query(
        "loop_b",
        Query::from("orders").where_eq("pid", qref("loop_a", "id")),
    );

    let mut outer = Batch::new();
    let over: FilterValue = param("ids");
    outer.for_each("loop", over, "row", inner.build());

    outer
        .try_build()
        .expect("for_each loop-body-internal $query refs must not reach the outer alias check");
}

// rv_query_builder_f3ed5a33_bind_outer_ref_still_validated
#[test]
fn rv_query_builder_f3ed5a33_bind_outer_ref_still_validated() {
    // A bind value IS a planner-counted outer-dependency position: a
    // reference to an unknown outer alias must still error.
    let mut bind = new_map();
    bind.insert("x".to_string(), qref("nope", "id"));

    let mut outer = Batch::new();
    outer.sub_batch("proc", Batch::new().build(), bind);

    match outer.try_build() {
        Err(BuildError::UnknownAlias { alias, .. }) => assert_eq!(alias, "nope"),
        other => panic!("expected UnknownAlias for bind ref to unknown alias, got {other:?}"),
    }
}

// rv_query_builder_f3ed5a33_batch_op_arm_is_typed_not_fallback
#[test]
fn rv_query_builder_f3ed5a33_batch_op_arm_is_typed_not_fallback() {
    // Guard the mechanism: a bind ref to a KNOWN outer alias keeps working
    // through the new typed arm (sanity that discovery still sees bind).
    let mut outer = Batch::new();
    let user = outer.query("user", Query::from("users"));
    let mut bind = new_map();
    bind.insert("uid".to_string(), user.first().field("id"));

    let mut inner = Batch::new();
    inner.query("data", Query::from("orders"));
    outer.sub_batch("proc", inner.build(), bind);

    let built = outer.build();
    match &built.queries.get("proc").expect("entry 'proc'").op {
        BatchOp::Batch(_) => {}
        other => panic!("expected BatchOp::Batch, got {other:?}"),
    }
    outer
        .try_build()
        .expect("bind ref to a known outer alias must resolve through the typed arm");
}
