//! Wire-contract tests: raw JSON is intentional because presence is the subject.

use shamir_query_types::filter::FilterValue;
use shamir_query_types::read::SelectItem;

use crate::{select, val};

/// Red before the fix: both aggregate variants lose explicit NULL on decode.
#[test]
fn builders_preserve_absent_null_and_expression_inputs_on_both_wires() {
    for (wire, absent, null, expression) in [
        (
            serde_json::json!({
                "type": "aggregate",
                "func": "count",
                "field": ["v"],
                "alias": "result",
                "distinct": false
            }),
            select::count("v", "result"),
            select::count_of(val::null(), "result"),
            select::count_of(val::col("v"), "result"),
        ),
        (
            serde_json::json!({
                "type": "aggregate_fn",
                "name": "median",
                "field": ["v"],
                "args": [],
                "alias": "result",
                "distinct": false
            }),
            select::agg_fn("median", "v", "result"),
            select::agg_fn_of("median", val::null(), "result"),
            select::agg_fn_of("median", val::col("v"), "result"),
        ),
    ] {
        for (input, item) in [
            (None, absent),
            (Some(serde_json::Value::Null), null),
            (
                Some(serde_json::json!({
                    "$ref": ["v"]
                })),
                expression,
            ),
        ] {
            let mut expected_wire = wire.clone();
            let mut expected_item = item.clone();
            // Expression builders use All as their placeholder; a real legacy
            // field here also proves that present input overrides that field.
            match &mut expected_item {
                SelectItem::Aggregate { field, .. } | SelectItem::AggregateFn { field, .. } => {
                    *field = shamir_query_types::read::AggregateField::Field(vec!["v".into()]);
                }
                other => panic!("expected aggregate, got {other:?}"),
            }
            if let Some(input) = input {
                expected_wire["input"] = input;
            }
            let json = serde_json::to_string(&expected_wire).unwrap();
            let decoded: SelectItem = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, expected_item);
            assert_eq!(serde_json::to_value(&decoded).unwrap(), expected_wire);
            assert_eq!(serde_json::to_value(&expected_item).unwrap(), expected_wire);
            assert_eq!(
                serde_json::from_str::<SelectItem>(
                    &serde_json::to_string(&expected_item).unwrap(),
                )
                .unwrap(),
                expected_item,
            );

            let bytes = rmp_serde::to_vec_named(&expected_wire).unwrap();
            let decoded: SelectItem = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(decoded, expected_item);
            let encoded = rmp_serde::to_vec_named(&expected_item).unwrap();
            assert_eq!(
                rmp_serde::from_slice::<serde_json::Value>(&encoded).unwrap(),
                expected_wire,
            );
            assert_eq!(
                rmp_serde::from_slice::<SelectItem>(&encoded).unwrap(),
                expected_item,
            );
            // Test the unmodified builder output as well as the field override.
            assert_eq!(
                serde_json::from_str::<SelectItem>(&serde_json::to_string(&item).unwrap()).unwrap(),
                item,
            );
            assert_eq!(
                rmp_serde::from_slice::<SelectItem>(&rmp_serde::to_vec_named(&item).unwrap())
                    .unwrap(),
                item,
            );
        }
    }
}

#[test]
fn literal_null_json_is_a_filter_value_not_an_absent_input() {
    assert_eq!(
        serde_json::from_str::<FilterValue>("null").unwrap(),
        FilterValue::Null,
    );
    assert_eq!(serde_json::to_string(&FilterValue::Null).unwrap(), "null");
}
