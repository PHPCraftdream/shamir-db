//! Builder output must retain literal NULL when decoded before execution.

use bytes::Bytes;
use shamir_query_builder::{select, val, Query};
use shamir_query_types::read::{GroupBy, Select};
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::filter::FilterContext;
use crate::query::read::aggregate::{apply_aggregate_all_with_context, try_apply_group_by};

fn records(interner: &Interner) -> Vec<(RecordId, Bytes)> {
    [1, 1, 2]
        .into_iter()
        .map(|group| {
            let mut record = new_map_wc(2);
            record.insert("group".to_string(), QueryValue::Int(group));
            record.insert("v".to_string(), QueryValue::Int(7));
            let inner = query_value_to_inner(&QueryValue::Map(record), interner).unwrap();
            (RecordId::new(), inner.to_bytes().unwrap())
        })
        .collect()
}

fn selection() -> Select {
    Query::from("rows")
        .select([
            select::count_of(val::null(), "null_count"),
            select::count_all("rows"),
            select::sum_of(val::null(), "null_sum"),
            select::avg_of(val::null(), "null_avg"),
            select::min_of(val::null(), "null_min"),
            select::max_of(val::null(), "null_max"),
            select::agg_fn_of("median", val::null(), "null_median"),
            select::count("v", "field_count"),
            select::sum("v", "field_sum"),
        ])
        .build()
        .select
}

fn assert_aggregates(result: &QueryValue, count: i64) {
    assert_eq!(result.get("null_count"), Some(&QueryValue::Int(0)));
    assert_eq!(result.get("rows"), Some(&QueryValue::Int(count)));
    assert_eq!(result.get("field_count"), Some(&QueryValue::Int(count)));
    assert_eq!(result.get("field_sum"), Some(&QueryValue::Int(7 * count)));
    for name in [
        "null_sum",
        "null_avg",
        "null_min",
        "null_max",
        "null_median",
    ] {
        assert_eq!(result.get(name), Some(&QueryValue::Null), "{name}");
    }
}

/// Red before the fix: wire decoding turns COUNT(NULL) into COUNT(All)=3;
/// even without wire loss the old SUM accumulator's empty identity is 0.
#[test]
fn ungrouped_literal_null_survives_builder_wire_and_execution() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let built = selection();
    let wire = rmp_serde::to_vec_named(&built).unwrap();
    let decoded: Select = rmp_serde::from_slice(&wire).unwrap();
    let records = records(&interner);
    for select in [&decoded, &built] {
        let result = apply_aggregate_all_with_context(&records, select, &interner, &ctx).unwrap();
        assert_eq!(result.len(), 1);
        assert_aggregates(&result[0], 3);
    }
}

/// Red before the fix: each group's COUNT(NULL) counts its rows after decode,
/// and SUM(NULL) returns 0 instead of NULL.
#[test]
fn grouped_literal_null_survives_builder_wire_and_execution() {
    let interner = Interner::default();
    let refs = new_map_wc(0);
    let ctx = FilterContext::new(&interner, &refs);
    let built = selection();
    let wire = rmp_serde::to_vec_named(&built).unwrap();
    let decoded: Select = rmp_serde::from_slice(&wire).unwrap();
    let records = records(&interner);
    for select in [&decoded, &built] {
        let result =
            try_apply_group_by(&records, &GroupBy::new(["group"]), select, &interner, &ctx)
                .unwrap();
        assert_eq!(result.len(), 2);
        for (group, count) in [(1, 2), (2, 1)] {
            let row = result
                .iter()
                .find(|row| row.get("group") == Some(&QueryValue::Int(group)))
                .unwrap();
            assert_aggregates(row, count);
        }
    }
}
