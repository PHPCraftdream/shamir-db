//! Wire-contract and rejection tests for FilterValue decoding.

use crate::filter::{Cond, Filter, FilterExpr, FilterValue, FnCall};
use serde::de::{Deserializer, Visitor};
use serde::Deserialize;
use serde_json::{json, Value};

#[test]
fn marker_wire_shapes_and_round_trips() {
    let cases = vec![
        (
            json!({
                "$ref": ["a", "b"]
            }),
            FilterValue::FieldRef {
                path: vec!["a".into(), "b".into()],
            },
        ),
        (
            json!({
                "$query": "q"
            }),
            FilterValue::query_ref("q"),
        ),
        (
            json!({
                "$query": "q",
                "path": "x"
            }),
            FilterValue::query_ref_with_path("q", "x"),
        ),
        (
            json!({
                "$param": "p"
            }),
            FilterValue::Param { name: "p".into() },
        ),
        (
            json!({
                "$fn": "NOW"
            }),
            FilterValue::FnCall {
                call: FnCall::simple("NOW"),
            },
        ),
        (
            json!({
                "$fn": {
                    "name": "F",
                    "args": [1]
                }
            }),
            FilterValue::FnCall {
                call: FnCall::complex("F", vec![FilterValue::Int(1)]),
            },
        ),
        (
            json!({
                "$expr": {
                    "op": "add",
                    "args": [1]
                }
            }),
            FilterValue::Expr {
                expr: FilterExpr::add(vec![FilterValue::Int(1)]),
            },
        ),
        (
            json!({
                "$cond": {
                    "if": {
                        "op": "eq",
                        "field": ["a"],
                        "value": 1
                    },
                    "then": true,
                    "else": false
                }
            }),
            FilterValue::Cond {
                cond: Box::new(Cond::new(
                    Filter::Eq {
                        field: vec!["a".into()],
                        value: FilterValue::Int(1),
                    },
                    FilterValue::Bool(true),
                    FilterValue::Bool(false),
                )),
            },
        ),
    ];
    for (wire, expected) in cases {
        assert_eq!(
            serde_json::from_value::<FilterValue>(wire.clone()).unwrap(),
            expected
        );
        let bytes = rmp_serde::to_vec_named(&wire).unwrap();
        let buffered: shamir_types::types::value::QueryValue =
            rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(
            FilterValue::try_from_query_value_ref(&buffered).unwrap(),
            expected
        );
        assert_eq!(
            rmp_serde::from_slice::<FilterValue>(&bytes).unwrap(),
            expected
        );
        assert_eq!(serde_json::to_value(&expected).unwrap(), wire);
        assert_eq!(
            serde_json::from_str::<FilterValue>(&serde_json::to_string(&expected).unwrap())
                .unwrap(),
            expected
        );
        assert_eq!(
            rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec_named(&expected).unwrap())
                .unwrap(),
            expected
        );
    }
}

#[test]
fn named_messagepack_marker_serialization_matches_wire_bytes() {
    let cases: [(FilterValue, &[u8]); 8] = [
        (FilterValue::field_ref("a"), b"\x81\xa4$ref\x91\xa1a"),
        (FilterValue::query_ref("q"), b"\x81\xa6$query\xa1q"),
        (FilterValue::query_ref_with_path("q", "x"), b"\x82\xa6$query\xa1q\xa4path\xa1x"),
        (FilterValue::Param { name: "p".into() }, b"\x81\xa6$param\xa1p"),
        (FilterValue::FnCall { call: FnCall::simple("NOW") }, b"\x81\xa3$fn\xa3NOW"),
        (FilterValue::FnCall { call: FnCall::complex("F", vec![FilterValue::Int(1)]) }, b"\x81\xa3$fn\x82\xa4name\xa1F\xa4args\x91\x01"),
        (FilterValue::Expr { expr: FilterExpr::add(vec![FilterValue::Int(1)]) }, b"\x81\xa5$expr\x82\xa2op\xa3add\xa4args\x91\x01"),
        (FilterValue::Cond { cond: Box::new(Cond::new(Filter::Eq { field: vec!["a".into()], value: FilterValue::Int(1) }, FilterValue::Bool(true), FilterValue::Bool(false))) }, b"\x81\xa5$cond\x83\xa2if\x83\xa2op\xa2eq\xa5field\x91\xa1a\xa5value\x01\xa4then\xc3\xa4else\xc2"),
    ];
    for (value, expected_bytes) in cases {
        assert_eq!(rmp_serde::to_vec_named(&value).unwrap(), expected_bytes);
    }
}

#[test]
fn literals_and_binary_preserve_wire_types() {
    for value in [
        FilterValue::Null,
        FilterValue::Bool(true),
        FilterValue::Int(-9),
        FilterValue::Float(1.5),
        FilterValue::String("text".into()),
        FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Bool(false)]),
    ] {
        assert_eq!(
            serde_json::from_str::<FilterValue>(&serde_json::to_string(&value).unwrap()).unwrap(),
            value
        );
        assert_eq!(
            rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec_named(&value).unwrap())
                .unwrap(),
            value
        );
    }
    let value = FilterValue::Binary(vec![1, 2, 3]);
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec_named(&value).unwrap()).unwrap(),
        value
    );
    // JSON represents bytes as an array, not a distinct binary wire type.
    assert_eq!(
        serde_json::from_str::<FilterValue>(&serde_json::to_string(&value).unwrap()).unwrap(),
        FilterValue::Array(vec![1.into(), 2.into(), 3.into()])
    );
}

#[test]
fn query_path_order_and_null() {
    for input in [
        r#"{"path":"x","$query":"q"}"#,
        r#"{"$query":"q","path":"x"}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<FilterValue>(input).unwrap(),
            FilterValue::query_ref_with_path("q", "x")
        );
    }
    for input in [
        r#"{"path":null,"$query":"q"}"#,
        r#"{"$query":"q","path":null}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<FilterValue>(input).unwrap(),
            FilterValue::query_ref("q")
        );
    }
    // Manually encoded MessagePack keeps the path-first ordering.
    for bytes in [
        &b"\x82\xa4path\xa1x\xa6$query\xa1q"[..],
        &b"\x82\xa6$query\xa1q\xa4path\xa1x"[..],
    ] {
        assert_eq!(
            rmp_serde::from_slice::<FilterValue>(bytes).unwrap(),
            FilterValue::query_ref_with_path("q", "x")
        );
    }
    let bytes = b"\x82\xa4path\xc0\xa6$query\xa1q";
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(bytes).unwrap(),
        FilterValue::query_ref("q")
    );
}

#[test]
fn unsigned_boundaries_remain_exact() {
    for (number, expected) in [
        (i64::MAX as u64, FilterValue::Int(i64::MAX)),
        (
            i64::MAX as u64 + 1,
            FilterValue::String((i64::MAX as u64 + 1).to_string()),
        ),
        (u64::MAX, FilterValue::String(u64::MAX.to_string())),
    ] {
        assert_eq!(
            serde_json::from_str::<FilterValue>(&number.to_string()).unwrap(),
            expected
        );
        assert_eq!(
            rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec(&number).unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn rejects_ambiguous_or_invalid_marker_maps() {
    // The first two cases exposed discriminator fallback before this fix.
    // The empty map was already rejected; it remains a rejection control.
    let cases = [
        (r#"{"$ref":42,"$query":"q"}"#, "$ref"),
        (r#"{"$query":"q","$ref":["x"]}"#, "$ref"),
        (r#"{"$ref":["a"],"path":["b"]}"#, "path"),
        // Well-typed `path` next to a non-query marker must still be refused.
        (r#"{"$ref":["a"],"path":"b"}"#, "path"),
        (r#"{"$param":"x","path":"b"}"#, "path"),
        (r#"{"$ref":["a"],"path":null}"#, "path"),
        (r#"{"$query":"q","extra":1}"#, "extra"),
        (r#"{}"#, "discriminator"),
        (r#"{"path":null}"#, "path"),
        (r#"{"$param":1}"#, "$param"),
        (r#"{"$ref":42}"#, "$ref"),
        (r#"{"$query":42}"#, "$query"),
        (r#"{"$fn":42}"#, "$fn"),
        (r#"{"$expr":42}"#, "$expr"),
        (r#"{"$cond":42}"#, "$cond"),
        (r#"{"$unknown":1}"#, "$unknown"),
        (r#"{"$query":"a","$query":"b"}"#, "$query"),
        (r#"{"path":null,"path":null,"$query":"q"}"#, "path"),
    ];
    for (input, key) in cases {
        let error = serde_json::from_str::<FilterValue>(input)
            .unwrap_err()
            .to_string();
        assert!(error.contains(key), "{input}: {error}");
        // Preserve duplicate keys for JSON above; other cases also exercise MessagePack maps.
        if !input.contains("\"$query\":\"a\",\"$query\"")
            && !input.contains("\"path\":null,\"path\"")
        {
            let wire: Value = serde_json::from_str(input).unwrap();
            let error =
                rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec_named(&wire).unwrap())
                    .unwrap_err()
                    .to_string();
            assert!(error.contains(key), "{input}: {error}");
        }
    }
}

/// A deserializer whose only callback is `visit_none`, which serde_json and
/// rmp_serde never produce for a bare nil but other formats do.
struct NoneDeserializer;

impl<'de> Deserializer<'de> for NoneDeserializer {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_none()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

#[test]
fn visit_none_callback_decodes_as_null() {
    assert_eq!(
        FilterValue::deserialize(NoneDeserializer).unwrap(),
        FilterValue::Null
    );
}
