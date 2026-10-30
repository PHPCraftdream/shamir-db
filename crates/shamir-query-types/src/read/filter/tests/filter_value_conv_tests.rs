//! Unit tests for `query_value_to_filter_value` and `filter_value_to_query_value`.
//!
//! Verifies:
//! - Literal variants convert directly (no msgpack).
//! - `List` converts recursively.
//! - Exotic variants (`Map`, `Set`, `Dec`, `Big`) → `None`.
//! - Symmetric round-trip: `query_value_to_filter_value(filter_value_to_query_value(lit)) == lit`.
//! - `From<QueryValue> for FilterValue` uses the direct path for literals
//!   (no silent Null) and falls back to msgpack for Map/expression defaults.

use shamir_types::types::common::{new_map, TSet};
use shamir_types::types::value::QueryValue;

use crate::filter::filter_enum::Filter;
use crate::filter::filter_value::{filter_value_to_query_value, query_value_to_filter_value};
use crate::filter::FilterValue;

#[test]
fn filter_value_msgpack_wire_shapes_and_rejections() {
    let cases: &[(&[u8], FilterValue)] = &[
        (&[0xc0], FilterValue::Null),
        (&[0xc3], FilterValue::Bool(true)),
        (&[0xff], FilterValue::Int(-1)),
        (&[0x01], FilterValue::Int(1)),
        (&[0xca, 0x3f, 0xc0, 0x00, 0x00], FilterValue::Float(1.5)),
        (&[0xa1, b'a'], FilterValue::String("a".into())),
        (&[0xc4, 0x01, b'a'], FilterValue::Binary(vec![b'a'])),
        (&[0x91, 0x01], FilterValue::Array(vec![FilterValue::Int(1)])),
    ];
    for (wire, expected) in cases {
        let decoded: FilterValue = rmp_serde::from_slice(wire).unwrap();
        assert_eq!(&decoded, expected);
        let encoded = rmp_serde::to_vec_named(expected).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<FilterValue>(&encoded).unwrap(),
            *expected
        );
    }
    assert!(rmp_serde::from_slice::<FilterValue>(&[0x80]).is_err());
    assert!(rmp_serde::from_slice::<FilterValue>(&[0xc1]).is_err());
}

#[test]
fn rv_types_18ac0cfc_filter_value_numeric_wire_parity() {
    let large = u64::MAX;
    let bytes = [0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&bytes).unwrap(),
        FilterValue::String(large.to_string())
    );
    assert_eq!(
        rmp_serde::from_slice::<Vec<FilterValue>>(&rmp_serde::to_vec(&vec![large]).unwrap())
            .unwrap(),
        vec![FilterValue::String(large.to_string())]
    );
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec(&i64::MAX).unwrap()).unwrap(),
        FilterValue::Int(i64::MAX)
    );
    let array = vec![FilterValue::Int(1), FilterValue::String("x".to_string())];
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec(&array).unwrap()).unwrap(),
        FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::String("x".to_string())
        ])
    );
    for float in [1.5, -0.0, f64::NAN] {
        let value: FilterValue =
            rmp_serde::from_slice(&rmp_serde::to_vec(&float).unwrap()).unwrap();
        match value {
            FilterValue::Float(decoded) if float.is_nan() => assert!(decoded.is_nan()),
            FilterValue::Float(decoded) => assert_eq!(decoded.to_bits(), float.to_bits()),
            other => panic!("expected Float, got {other:?}"),
        }
    }
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&rmp_serde::to_vec(&1u64).unwrap()).unwrap(),
        FilterValue::Int(1)
    );
}

#[test]
fn qv_to_fv_null() {
    let result = query_value_to_filter_value(&QueryValue::Null);
    assert_eq!(result, Some(FilterValue::Null));
}

#[test]
fn qv_to_fv_bool_true() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Bool(true)),
        Some(FilterValue::Bool(true))
    );
}

#[test]
fn qv_to_fv_bool_false() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Bool(false)),
        Some(FilterValue::Bool(false))
    );
}

#[test]
fn qv_to_fv_int() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Int(42)),
        Some(FilterValue::Int(42))
    );
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Int(-999)),
        Some(FilterValue::Int(-999))
    );
}

#[test]
fn qv_to_fv_f64() {
    // Use 1.5 (exact in IEEE 754) to avoid clippy::approx_constant.
    let result = query_value_to_filter_value(&QueryValue::F64(1.5));
    match result {
        Some(FilterValue::Float(f)) => assert!((f - 1.5).abs() < 1e-10),
        other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn qv_to_fv_str() {
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Str("hello".to_string())),
        Some(FilterValue::String("hello".to_string()))
    );
}

#[test]
fn qv_to_fv_bin() {
    let bytes = vec![1u8, 2, 3];
    assert_eq!(
        query_value_to_filter_value(&QueryValue::Bin(bytes.clone())),
        Some(FilterValue::Binary(bytes))
    );
}

// ── recursive List conversion ────────────────────────────────────────────────

#[test]
fn qv_to_fv_list_recursive() {
    let qv = QueryValue::List(vec![
        QueryValue::Int(1),
        QueryValue::Str("x".to_string()),
        QueryValue::Bool(false),
    ]);
    let expected = FilterValue::Array(vec![
        FilterValue::Int(1),
        FilterValue::String("x".to_string()),
        FilterValue::Bool(false),
    ]);
    assert_eq!(query_value_to_filter_value(&qv), Some(expected));
}

#[test]
fn qv_to_fv_nested_list() {
    let inner = QueryValue::List(vec![QueryValue::Int(10), QueryValue::Int(20)]);
    let outer = QueryValue::List(vec![inner, QueryValue::Null]);
    let result = query_value_to_filter_value(&outer);
    let expected = FilterValue::Array(vec![
        FilterValue::Array(vec![FilterValue::Int(10), FilterValue::Int(20)]),
        FilterValue::Null,
    ]);
    assert_eq!(result, Some(expected));
}

// ── exotic variants → None ───────────────────────────────────────────────────

#[test]
fn qv_to_fv_map_returns_none() {
    let mut m = new_map();
    m.insert("$fn".to_string(), QueryValue::Str("now".to_string()));
    let qv = QueryValue::Map(m);
    // Map has no direct FilterValue equivalent → None (use msgpack fallback).
    assert!(query_value_to_filter_value(&qv).is_none());
}

#[test]
fn qv_to_fv_set_returns_none() {
    // Set has no direct FilterValue equivalent → None.
    let qv = QueryValue::Set(TSet::default());
    assert!(query_value_to_filter_value(&qv).is_none());
}

// ── symmetric round-trip ─────────────────────────────────────────────────────

/// For every literal FilterValue, the round-trip
/// `qv_to_fv(fv_to_qv(fv)) == Some(fv)` must hold.
#[test]
fn round_trip_literals_symmetric() {
    let literals: Vec<FilterValue> = vec![
        FilterValue::Null,
        FilterValue::Bool(true),
        FilterValue::Bool(false),
        FilterValue::Int(0),
        FilterValue::Int(i64::MIN),
        FilterValue::Int(i64::MAX),
        FilterValue::Float(0.0),
        FilterValue::Float(-1.5),
        FilterValue::String(String::new()),
        FilterValue::String("round-trip".to_string()),
        FilterValue::Binary(vec![]),
        FilterValue::Binary(vec![0xde, 0xad, 0xbe, 0xef]),
        FilterValue::Array(vec![FilterValue::Int(1), FilterValue::Bool(false)]),
    ];

    for fv in &literals {
        let qv = filter_value_to_query_value(fv)
            .unwrap_or_else(|| panic!("filter_value_to_query_value returned None for {:?}", fv));
        let back = query_value_to_filter_value(&qv)
            .unwrap_or_else(|| panic!("query_value_to_filter_value returned None for {:?}", qv));
        assert_eq!(&back, fv, "round-trip failed for {:?}", fv);
    }
}

// ── From<QueryValue> for FilterValue ────────────────────────────────────────

#[test]
fn from_qv_literal_is_direct_not_null() {
    // Regression: before this fix, From<QueryValue> used msgpack+unwrap_or(Null).
    // A valid literal must NOT become Null.
    let cases = vec![
        (QueryValue::Bool(true), FilterValue::Bool(true)),
        (QueryValue::Int(99), FilterValue::Int(99)),
        (
            QueryValue::Str("abc".to_string()),
            FilterValue::String("abc".to_string()),
        ),
        (QueryValue::Null, FilterValue::Null),
    ];
    for (qv, expected) in cases {
        let got = FilterValue::from(qv.clone());
        assert_eq!(
            got, expected,
            "From<QueryValue>({:?}) gave wrong result",
            qv
        );
    }
}

#[test]
fn from_qv_list_converts_recursively() {
    let qv = QueryValue::List(vec![QueryValue::Int(5), QueryValue::Bool(true)]);
    let got = FilterValue::from(qv);
    assert_eq!(
        got,
        FilterValue::Array(vec![FilterValue::Int(5), FilterValue::Bool(true)])
    );
}

#[test]
fn fallible_conversion_preserves_numeric_and_set_wire_projection() {
    let big = QueryValue::Big(num_bigint::BigInt::from(u64::MAX));
    let decimal = QueryValue::Dec("123.4500".parse().unwrap());
    let mut set = TSet::default();
    set.insert(QueryValue::Int(7));
    set.insert(QueryValue::Str("x".to_owned()));
    let set = QueryValue::Set(set);
    for value in [big, decimal, set] {
        let expected: FilterValue =
            rmp_serde::from_slice(&rmp_serde::to_vec_named(&value).unwrap()).unwrap();
        assert_eq!(
            FilterValue::try_from_query_value_ref(&value).unwrap(),
            expected
        );
    }
    assert_eq!(
        FilterValue::try_from_query_value(QueryValue::Big(num_bigint::BigInt::from(u64::MAX)))
            .unwrap(),
        FilterValue::String(u64::MAX.to_string())
    );
}

#[test]
fn fallible_conversion_rejects_maps_even_inside_lists() {
    let unsupported = shamir_types::mpack!({
        "ordinary": 1
    });
    assert!(FilterValue::try_from_query_value(unsupported.clone()).is_err());
    assert!(FilterValue::try_from_query_value(QueryValue::List(vec![unsupported])).is_err());
    let conflicting = shamir_types::mpack!({
        "$query": "a",
        "$ref": "b"
    });
    assert!(FilterValue::try_from_query_value(conflicting.clone()).is_err());
    assert!(
        FilterValue::try_from_query_value(QueryValue::List(vec![QueryValue::List(vec![
            conflicting
        ])]))
        .is_err()
    );
    let valid = shamir_types::mpack!({
        "$query": "a"
    });
    assert_eq!(
        FilterValue::try_from_query_value(valid).unwrap(),
        FilterValue::query_ref("a")
    );
}

// ── #983 round 2: FilterValue::Binary misclassified as String on real wire
//    input (untagged-enum variant-order ambiguity) ──────────────────────────
//
// `FilterValue` is `#[serde(untagged)]`. Before the fix, `String` was
// declared BEFORE `Binary`; the stdlib `String` Deserialize impl silently
// accepts `visit_bytes`/`visit_byte_buf` as a fallback, so a genuine
// msgpack bin8/16/32 payload whose bytes happen to be valid UTF-8 (e.g.
// `[1, 2, 3]` — every byte <= 0x7F) was captured by the `String` arm and
// never reached `Binary` at all. Reordering `Binary` before `String` fixes
// this because untagged enums try variants in declaration order and take
// the first successful match.

/// The exact real-wire-bytes regression: bytes captured from a genuine JS
/// client encode of
/// `Query.from('t').where(filter.eq('blob', filter.bin([1,2,3]))).build()`
/// via `@msgpack/msgpack`'s `encode()` (re-verified this session with a
/// `node -e` one-liner using the same library — see brief
/// `docs/dev-artifacts/prompts/bugfix-983/04-fix-filtervalue-binary-string-ambiguity.md`
/// for the exact reproduction). The `value` field is unambiguously a
/// msgpack **bin8** marker (`c4 03 01 02 03`), never a string.
///
/// FAILS before the fix: decodes to `FilterValue::String("\u{1}\u{2}\u{3}")`
/// (because those 3 bytes happen to be valid UTF-8).
/// PASSES after the fix: decodes to `FilterValue::Binary(vec![1, 2, 3])`.
#[test]
fn real_wire_bytes_bin8_payload_decodes_as_binary_not_string() {
    let hex = "83a26f70a26571a56669656c6491a4626c6f62a576616c7565c403010203";
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();

    let filter: Filter =
        rmp_serde::from_slice(&bytes).expect("real-wire Eq filter bytes must deserialize");

    match filter {
        Filter::Eq { field, value } => {
            assert_eq!(field, vec!["blob".to_string()]);
            assert_eq!(
                value,
                FilterValue::Binary(vec![1, 2, 3]),
                "expected Binary([1,2,3]); a String variant here means the \
                 untagged-enum ordering bug has regressed"
            );
        }
        other => panic!("expected Filter::Eq, got {other:?}"),
    }
}

/// Round-trip a `Filter::Eq` with a `Binary` value whose bytes are ALL in
/// the valid-UTF8 ASCII range (`[1, 2, 3]`) — the exact shape that used to
/// be misclassified as `String`. Must decode back as `Binary`.
#[test]
fn binary_value_all_ascii_range_round_trips_as_binary() {
    let f = Filter::Eq {
        field: vec!["blob".to_string()],
        value: FilterValue::Binary(vec![1, 2, 3]),
    };
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let f2: Filter = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(f, f2);
    match f2 {
        Filter::Eq { value, .. } => assert_eq!(value, FilterValue::Binary(vec![1, 2, 3])),
        other => panic!("expected Filter::Eq, got {other:?}"),
    }
}

/// Round-trip a `Filter::Eq` with a `Binary` value containing invalid-UTF8
/// bytes — this direction already worked before the fix (invalid UTF-8
/// naturally falls through `String`'s Deserialize), but assert it explicitly
/// so the fix isn't accidentally UTF8-payload-specific in either direction.
#[test]
fn binary_value_invalid_utf8_round_trips_as_binary() {
    let f = Filter::Eq {
        field: vec!["blob".to_string()],
        value: FilterValue::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]),
    };
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let f2: Filter = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(f, f2);
    match f2 {
        Filter::Eq { value, .. } => {
            assert_eq!(value, FilterValue::Binary(vec![0xDE, 0xAD, 0xBE, 0xEF]))
        }
        other => panic!("expected Filter::Eq, got {other:?}"),
    }
}

/// Direction-reversal check: a genuine `String` filter value must still
/// decode as `FilterValue::String` after the `Binary`/`String` reorder.
/// This holds because a real JS string always encodes as msgpack
/// str8/fixstr/str16/str32 — a distinct wire type from bin8/16/32 — and
/// `serde_bytes`'s `Deserialize` for `Vec<u8>` does NOT accept
/// `visit_str`/`visit_string` (unlike the reverse asymmetry that caused
/// this bug), so there is no new String-vs-Binary collision in this
/// direction.
#[test]
fn string_value_still_decodes_as_string_after_reorder() {
    let f = Filter::Eq {
        field: vec!["tag".to_string()],
        value: FilterValue::String("hello".to_string()),
    };
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let f2: Filter = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(f, f2);
    match f2 {
        Filter::Eq { value, .. } => assert_eq!(value, FilterValue::String("hello".to_string())),
        other => panic!("expected Filter::Eq, got {other:?}"),
    }
}

/// A real JS ARRAY (not a `Uint8Array`) must still decode as
/// `FilterValue::Array`, not `Binary` — msgpack array markers and bin8/16/32
/// markers are structurally distinct wire types, so this collision is
/// expected to be a non-issue, but verify it explicitly per the brief's
/// due-diligence requirement rather than assuming.
#[test]
fn array_value_still_decodes_as_array_not_binary() {
    let f = Filter::Eq {
        field: vec!["tags".to_string()],
        value: FilterValue::Array(vec![
            FilterValue::Int(1),
            FilterValue::Int(2),
            FilterValue::Int(3),
        ]),
    };
    let bytes = rmp_serde::to_vec_named(&f).unwrap();
    let f2: Filter = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(f, f2);
    match f2 {
        Filter::Eq { value, .. } => assert_eq!(
            value,
            FilterValue::Array(vec![
                FilterValue::Int(1),
                FilterValue::Int(2),
                FilterValue::Int(3)
            ])
        ),
        other => panic!("expected Filter::Eq, got {other:?}"),
    }
}

#[test]
fn rv_types_18ac0cfc_oversized_array_element_decodes_as_decimal_string() {
    let mut bytes = vec![0x91];
    bytes.extend_from_slice(&[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(
        rmp_serde::from_slice::<FilterValue>(&bytes).unwrap(),
        FilterValue::Array(vec![FilterValue::String(u64::MAX.to_string())])
    );
}
