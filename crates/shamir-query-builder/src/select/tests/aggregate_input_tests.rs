use shamir_query_types::read::{AggFunc, AggregateField, SelectItem};

use crate::select;
use crate::val::{col, lit, mul, param, qref};

#[test]
fn expression_builders_keep_input_separate_from_factory_args() {
    let input = mul(col("v"), qref("q1", "[0].m"));
    let items = [
        select::sum_of(input.clone(), "total"),
        select::avg_of(input.clone(), "mean"),
        select::min_of(input.clone(), "low"),
        select::max_of(input.clone(), "high"),
        select::count_of(input.clone(), "count"),
    ];
    for (item, expected) in items.into_iter().zip([
        AggFunc::Sum,
        AggFunc::Avg,
        AggFunc::Min,
        AggFunc::Max,
        AggFunc::Count,
    ]) {
        match item {
            SelectItem::Aggregate {
                func,
                field,
                input: actual,
                distinct,
                ..
            } => {
                assert_eq!(func, expected);
                assert_eq!(field, AggregateField::All);
                assert_eq!(actual, Some(input.clone()));
                assert!(!distinct);
            }
            other => panic!("expected aggregate, got {other:?}"),
        }
    }
    let item = select::agg_fn_with_args_of("percentile", input.clone(), "p", [lit(0.9)]);
    match item {
        SelectItem::AggregateFn {
            input: actual,
            args,
            ..
        } => {
            assert_eq!(actual, Some(input));
            assert_eq!(args, vec![lit(0.9)]);
        }
        other => panic!("expected library aggregate, got {other:?}"),
    }
}

#[test]
fn expression_aggregates_roundtrip_json_and_named_msgpack() {
    let input = mul(col("price"), param("rate"));
    for item in [
        select::avg_of(input.clone(), "mean"),
        select::agg_distinct_of(AggFunc::Sum, input.clone(), "total"),
        select::agg_fn_of("median", input.clone(), "median"),
        select::agg_fn_distinct_of("array_agg", input.clone(), "values"),
        select::agg_fn_with_args_of("percentile", input, "p", [lit(0.5)]),
    ] {
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"input\":"));
        assert_eq!(serde_json::from_str::<SelectItem>(&json).unwrap(), item);
        let bytes = rmp_serde::to_vec_named(&item).unwrap();
        assert_eq!(rmp_serde::from_slice::<SelectItem>(&bytes).unwrap(), item);
    }
}

#[test]
fn old_aggregate_wire_payloads_are_unchanged() {
    // Raw JSON is intentional: these are legacy wire compatibility vectors.
    let fixtures = [
        (
            r#"{
                "type": "aggregate",
                "func": "sum",
                "field": ["v"],
                "alias": "total",
                "distinct": false
            }"#,
            select::sum("v", "total"),
        ),
        (
            r#"{
                "type": "aggregate_fn",
                "name": "median",
                "field": ["v"],
                "args": [],
                "alias": "middle",
                "distinct": false
            }"#,
            select::agg_fn("median", "v", "middle"),
        ),
    ];
    for (wire, expected) in fixtures {
        let decoded: SelectItem = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, expected);
        let legacy: serde_json::Value = serde_json::from_str(wire).unwrap();
        assert_eq!(serde_json::to_value(&expected).unwrap(), legacy);
        let bytes = rmp_serde::to_vec_named(&legacy).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<SelectItem>(&bytes).unwrap(),
            expected
        );
    }
}

#[test]
fn explicit_null_input_retains_legacy_field_semantics() {
    // Raw JSON tests the intentional absent/null normalization of input.
    let wire = r#"{
        "type": "aggregate",
        "func": "sum",
        "field": ["v"],
        "input": null,
        "alias": "total"
    }"#;
    let item: SelectItem = serde_json::from_str(wire).unwrap();
    assert_eq!(item, select::sum("v", "total"));
    let encoded = serde_json::to_value(item).unwrap();
    assert!(!encoded.as_object().unwrap().contains_key("input"));
}
