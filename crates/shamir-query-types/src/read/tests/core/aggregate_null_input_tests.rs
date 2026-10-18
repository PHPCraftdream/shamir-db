//! Aggregate input presence on the MessagePack wire.

use crate::filter::FilterValue;
use crate::read::{AggFunc, AggregateField, SelectItem};

#[test]
fn literal_null_decodes_as_filter_value() {
    // MessagePack nil is the independent wire oracle for literal NULL.
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&[0xc0]).unwrap(),
        FilterValue::Null,
    );
    assert_eq!(rmp_serde::to_vec_named(&FilterValue::Null).unwrap(), [0xc0]);
}

/// Red before the presence deserializer: Some(Null) decodes as None.
#[test]
fn aggregate_input_states_roundtrip_msgpack() {
    for input in [
        None,
        Some(FilterValue::Null),
        Some(FilterValue::field_ref("v")),
    ] {
        for item in [
            SelectItem::Aggregate {
                func: AggFunc::Count,
                field: AggregateField::Field(vec!["v".into()]),
                input: input.clone(),
                alias: None,
                distinct: false,
            },
            SelectItem::AggregateFn {
                name: "median".into(),
                field: AggregateField::Field(vec!["v".into()]),
                input: input.clone(),
                args: Vec::new(),
                alias: None,
                distinct: false,
            },
        ] {
            let bytes = rmp_serde::to_vec_named(&item).unwrap();
            let decoded: SelectItem = rmp_serde::from_slice(&bytes).unwrap();
            assert_eq!(decoded, item);
        }
    }
}
