//! Regression test for RV-QUERY-BUILDER-224EF7B1: `collect_query_refs` must
//! use the planner's reserved-key marker convention (a len-1 map keyed
//! `$query`/`$fn`/`$cond`/`$expr`, or a len-2 `$query`+`path` map — see
//! `shamir-query-types` `ops/batch/plan/planner.rs`,
//! `extract_deps_from_value`; a len-2 `$query`+`path` marker additionally
//! requires `path` to be a STRING, mirroring the planner's typed decode
//! `QueryRef { path: Option<String> }`), not the loose "any map containing
//! `$query`" pre-#641 rule.

use shamir_collections::new_map;
use shamir_query_types::write::SetOp;
use shamir_query_types::TableRef;
use shamir_types::types::value::QueryValue;

use crate::batch::build_error::BuildError;
use crate::batch::Batch;

fn set_op_with_value(value: QueryValue) -> SetOp {
    SetOp {
        set: TableRef::new("cache"),
        key: QueryValue::Str("k".to_string()),
        value,
    }
}

// rv_query_builder_224ef7b1_literal_query_key_map_is_not_a_marker
#[test]
fn rv_query_builder_224ef7b1_literal_query_key_map_is_not_a_marker() {
    // A write-value map with a literal "$query" key plus an ordinary sibling
    // is literal DATA to the planner (len-2 map without "path" is not a
    // marker) — it must not produce a false UnknownAlias in try_build.
    let mut m = new_map();
    m.insert(
        "$query".to_string(),
        QueryValue::Str("@missing".to_string()),
    );
    m.insert("ordinary".to_string(), QueryValue::Int(1));

    let mut outer = Batch::new();
    outer.upsert("kv", set_op_with_value(QueryValue::Map(m)));

    outer
        .try_build()
        .expect("a multi-key map merely containing a \"$query\" key is literal data, not a marker");
}

// rv_query_builder_224ef7b1_genuine_marker_still_recognized
#[test]
fn rv_query_builder_224ef7b1_genuine_marker_still_recognized() {
    // Recognition must be preserved: an exact 1-key `{"$query": "@missing"}`
    // marker IS an outer ref to an unknown alias and must still error.
    let mut m = new_map();
    m.insert(
        "$query".to_string(),
        QueryValue::Str("@missing".to_string()),
    );

    let mut outer = Batch::new();
    outer.upsert("kv", set_op_with_value(QueryValue::Map(m)));

    match outer.try_build() {
        Err(BuildError::UnknownAlias { alias, .. }) => assert_eq!(alias, "missing"),
        other => panic!("expected UnknownAlias for a genuine $query marker, got {other:?}"),
    }
}

// rv_query_builder_224ef7b1_non_string_path_not_collected
#[test]
fn rv_query_builder_224ef7b1_non_string_path_not_collected() {
    // The planner's typed decode (`QueryRef { path: Option<String> }`)
    // accepts only a STRING `path`; a len-2 `{"$query", "path"}` map with a
    // non-string `path` is therefore literal write data, not a marker — it
    // must not be collected as a `$query` ref (no UnknownAlias).
    let mut m = new_map();
    m.insert(
        "$query".to_string(),
        QueryValue::Str("@missing".to_string()),
    );
    m.insert("path".to_string(), QueryValue::Int(5));

    let mut outer = Batch::new();
    outer.upsert("kv", set_op_with_value(QueryValue::Map(m)));

    outer
        .try_build()
        .expect("a len-2 map with a non-string `path` is literal data, not a marker");
}
