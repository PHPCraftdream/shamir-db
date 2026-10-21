//! Cross-path Int/F64 numeric-comparison regression tests — F-2 (#791).
//!
//! Before the F-2 consolidation, the exact bounds-check + floor/fract
//! `cmp_i64_f64` technique (CR-D3, #784) existed as two byte-for-byte
//! duplicated copies (`resolve.rs`, `read/order.rs`), while three OTHER
//! numeric-comparison sites in this crate — the bytes-level pre-filter
//! (`eval_bytes.rs::compare_raw_to_filter`), `IN`/`NOT IN` set membership
//! (`filter_node.rs::set_contains_coercing`), and MIN/MAX aggregation
//! (`read/aggregate.rs::OwnedScalar::cmp_scalar`) — still used the old lossy
//! `as f64`/`as i64` cast technique. A filter's answer must not depend on
//! which fast path evaluated it.
//!
//! Each test below takes ONE `(i64, f64)` edge-case pair and asserts that
//! EVERY fast path agrees on the same `Eq`/`Lt`/`Gt` answer:
//!
//! - the general evaluator (`Filter::Eq`/`Gt`/`Lt` via `compile_filter` +
//!   `matches`, which resolves through `scalar_ref_cmp_qv`/`compare_values`),
//! - the bytes pre-filter (`FilterNode::matches_msgpack_bytes`),
//! - `IN` set membership (`Filter::In` with all-literal values, which
//!   compiles to the `InSet` fast path and `set_contains_coercing`),
//! - MIN/MAX aggregation (`apply_aggregate_all` with `select::min`/`max`),
//! - ORDER BY (`apply_order_by_qv`'s `QvSortKey` comparator).

use bytes::Bytes;
use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_builder::select;
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc, TMap};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval::compile_filter;
use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::{apply_aggregate_all, apply_order_by_qv, OrderBy, Select};

/// One Int/F64 edge-case pair plus its expected ordering relationship
/// (`int_value.cmp_exact(float_value)`), computed by hand from the exact
/// mathematical values (NOT from any lossy cast).
struct Case {
    name: &'static str,
    int_value: i64,
    float_value: f64,
    /// true if `int_value == float_value` exactly (mathematically).
    exactly_equal: bool,
    /// true if `int_value < float_value` exactly (mathematically).
    int_less_than_float: bool,
}

fn cases() -> Vec<Case> {
    vec![
        // Negative fractional: -5 vs -4.5 -- int < float. A naive
        // `f.fract() > 0.0` test would see `(-4.5_f64).fract() == -0.5`
        // (negative, sign-preserving truncation) and wrongly conclude "no
        // fractional part" territory; the exact floor-based technique does
        // not have this bug.
        Case {
            name: "negative_fractional",
            int_value: -5,
            float_value: -4.5,
            exactly_equal: false,
            int_less_than_float: true,
        },
        // 2^53 boundary, exact case: both sides are exactly 2^53.
        Case {
            name: "two_pow_53_exact_equal",
            int_value: 1i64 << 53,
            float_value: (1i64 << 53) as f64,
            exactly_equal: true,
            int_less_than_float: false,
        },
        // 2^53 + 1: NOT exactly representable as f64 -- `(2^53 + 1) as f64`
        // rounds DOWN to `2^53.0`. The naive lossy cast would make this
        // compare Equal to `2^53.0`; the exact technique must report
        // `int_value > float_value` (9007199254740993 > 9007199254740992.0).
        Case {
            name: "two_pow_53_plus_one_vs_rounded_float",
            int_value: (1i64 << 53) + 1,
            float_value: (1i64 << 53) as f64,
            exactly_equal: false,
            int_less_than_float: false,
        },
        // 2^63 boundary: i64::MAX vs the f64 value 2^63 (which is OUT of
        // i64's range -- i64::MAX == 2^63 - 1). A naive
        // `f <= i64::MAX as f64` clamp wrongly passes here because
        // `i64::MAX as f64` itself rounds UP to exactly `2^63.0`.
        Case {
            name: "two_pow_63_boundary_int_max_vs_two_pow_63",
            int_value: i64::MAX,
            float_value: 9223372036854775808.0, // 2^63, exact f64 literal
            exactly_equal: false,
            int_less_than_float: true,
        },
        // i64::MIN vs the exact f64 -2^63 -- Equal at the lower boundary.
        Case {
            name: "i64_min_exactly_equal",
            int_value: i64::MIN,
            float_value: -9223372036854775808.0, // -2^63, exact f64 literal
            exactly_equal: true,
            int_less_than_float: false,
        },
    ]
}

// ── shared record/interning helpers ─────────────────────────────────────────

fn touch(i: &Interner, s: &str) -> shamir_types::core::interner::InternerKey {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    }
}

/// Build `{"v": Int(int_value)}` as an `InnerValue` (id-keyed) for the
/// general evaluator + bytes pre-filter paths.
fn make_inner_record(interner: &Interner, int_value: i64) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(touch(interner, "v"), InnerValue::Int(int_value));
    InnerValue::Map(m)
}

fn empty_refs() -> TMap<String, crate::query::read::QueryResult> {
    new_map()
}

// ── cross-path assertion ────────────────────────────────────────────────────

/// Assert that every fast path agrees on `Eq`/`Lt`/`Gt` for `case`.
fn assert_all_paths_agree(case: &Case) {
    let interner = Interner::new();
    let record = make_inner_record(&interner, case.int_value);
    let bytes: Bytes = record.to_bytes().expect("encode record to bytes");
    let refs = empty_refs();
    let ctx = FilterContext::new(&interner, &refs);

    for (op_name, op_filter, expected) in [
        (
            "Eq",
            Filter::Eq {
                field: vec!["v".to_string()],
                value: FilterValue::Float(case.float_value),
            },
            case.exactly_equal,
        ),
        (
            "Lt",
            Filter::Lt {
                field: vec!["v".to_string()],
                value: FilterValue::Float(case.float_value),
            },
            case.int_less_than_float,
        ),
        (
            "Gt",
            Filter::Gt {
                field: vec!["v".to_string()],
                value: FilterValue::Float(case.float_value),
            },
            !case.exactly_equal && !case.int_less_than_float,
        ),
    ] {
        let compiled = compile_filter(&op_filter, &interner);

        // Path 1: general evaluator.
        let general_result = compiled.matches(&record, &ctx);
        assert_eq!(
            general_result, expected,
            "[{}] general evaluator ({op_name}) disagrees: got {general_result}, want {expected}",
            case.name
        );

        // Path 2: bytes pre-filter. `None` means "fell back", not a
        // disagreement -- only a `Some` result must match.
        if let Some(bytes_result) = compiled.matches_msgpack_bytes(&bytes) {
            assert_eq!(
                bytes_result, expected,
                "[{}] bytes pre-filter ({op_name}) disagrees: got {bytes_result}, want {expected}",
                case.name
            );
        }
    }

    // Path 3: IN set membership (all-literal -> InSet fast path ->
    // `set_contains_coercing`). Only equality is meaningful for `$in`.
    let in_filter = Filter::In {
        field: vec!["v".to_string()],
        values: vec![FilterValue::Float(case.float_value)],
    };
    let in_compiled = compile_filter(&in_filter, &interner);
    let in_result = in_compiled.matches(&record, &ctx);
    assert_eq!(
        in_result, case.exactly_equal,
        "[{}] IN set membership disagrees: got {in_result}, want {}",
        case.name, case.exactly_equal
    );

    // Path 4: MIN/MAX aggregation. A two-row group {int_value, float_value}
    // -- MIN must be whichever is exactly smaller (or either, if equal);
    // MAX must be whichever is exactly larger (or either, if equal).
    let records: Vec<(RecordId, Bytes)> = vec![
        (
            RecordId::new(),
            make_query_value_bytes(&interner, QueryValue::Int(case.int_value)),
        ),
        (
            RecordId::new(),
            make_query_value_bytes(&interner, QueryValue::F64(case.float_value)),
        ),
    ];
    let select = Select {
        items: vec![select::min("v", "mn"), select::max("v", "mx")],
        distinct: false,
    };
    let agg_result = apply_aggregate_all(
        &records,
        &select,
        &interner,
        ScalarResolver::builtins_only(),
    );
    assert_eq!(agg_result.len(), 1);
    let (expected_min, expected_max) = if case.exactly_equal {
        // Either representation is an acceptable "the smallest/largest" —
        // but since Int is inserted first and the accumulator only takes a
        // NEW value on strict Greater(for Min)/Less(for Max), an exact tie
        // keeps the first-seen value: Int for both.
        (
            QueryValue::Int(case.int_value),
            QueryValue::Int(case.int_value),
        )
    } else if case.int_less_than_float {
        (
            QueryValue::Int(case.int_value),
            QueryValue::F64(case.float_value),
        )
    } else {
        (
            QueryValue::F64(case.float_value),
            QueryValue::Int(case.int_value),
        )
    };
    assert_eq!(
        agg_result[0]["mn"], expected_min,
        "[{}] MIN disagrees with the exact ordering",
        case.name
    );
    assert_eq!(
        agg_result[0]["mx"], expected_max,
        "[{}] MAX disagrees with the exact ordering",
        case.name
    );

    // Path 5: ORDER BY (`QvSortKey` comparator). Sort the same two-row
    // [Int, F64] set ASC; the exactly-smaller value must sort first.
    let mut qv_records = vec![
        make_field_record(QueryValue::Int(case.int_value)),
        make_field_record(QueryValue::F64(case.float_value)),
    ];
    let order = OrderBy::asc("v");
    apply_order_by_qv(&mut qv_records, &order);
    if case.exactly_equal {
        // Stable sort -- insertion order (Int first) preserved on a tie.
        assert_eq!(
            qv_records[0]["v"],
            QueryValue::Int(case.int_value),
            "[{}] ORDER BY: exact tie must preserve insertion order",
            case.name
        );
    } else {
        let expected_first = if case.int_less_than_float {
            QueryValue::Int(case.int_value)
        } else {
            QueryValue::F64(case.float_value)
        };
        assert_eq!(
            qv_records[0]["v"], expected_first,
            "[{}] ORDER BY ASC disagrees with the exact ordering",
            case.name
        );
    }
}

/// Encode a single `{"v": qv}` record to raw msgpack bytes via the id-keyed
/// `InnerValue` path (mirrors how `aggregate.rs` decodes group rows).
fn make_query_value_bytes(interner: &Interner, qv: QueryValue) -> Bytes {
    let inner = match qv {
        QueryValue::Int(i) => InnerValue::Int(i),
        QueryValue::F64(f) => InnerValue::F64(f),
        _ => unreachable!("test only feeds Int/F64"),
    };
    let mut m = new_map_wc(1);
    m.insert(touch(interner, "v"), inner);
    InnerValue::Map(m).to_bytes().expect("encode to bytes")
}

/// Build a `QueryValue::Map` record `{"v": qv}` for the ORDER BY path (which
/// is name-keyed, not id-keyed).
fn make_field_record(qv: QueryValue) -> QueryValue {
    let mut m: indexmap::IndexMap<String, QueryValue, shamir_collections::THasher> = new_map_wc(1);
    m.insert("v".to_string(), qv);
    QueryValue::Map(m)
}

#[test]
fn cross_path_negative_fractional() {
    assert_all_paths_agree(&cases()[0]);
}

#[test]
fn cross_path_two_pow_53_exact_equal() {
    assert_all_paths_agree(&cases()[1]);
}

#[test]
fn cross_path_two_pow_53_plus_one_vs_rounded_float() {
    assert_all_paths_agree(&cases()[2]);
}

#[test]
fn cross_path_two_pow_63_boundary() {
    assert_all_paths_agree(&cases()[3]);
}

#[test]
fn cross_path_i64_min_exactly_equal() {
    assert_all_paths_agree(&cases()[4]);
}

// ============================================================================
// u64 vs F64 -- the bytes pre-filter's dedicated unsigned lane
// (`RawScalar::U64`), which has no counterpart in the general evaluator or
// ORDER BY (both work off `QueryValue::Int(i64)`, never a raw `u64`).
// ============================================================================

/// Build a raw msgpack record whose `"v"` field is encoded as msgpack
/// `uint64` (`0xcf`) -- i.e. a value that only fits `u64`, not `i64` --
/// so the bytes pre-filter decodes it as `RawScalar::U64`, not `I64`.
fn encode_u64_field_record(interner: &Interner, u: u64) -> Bytes {
    let key = touch(interner, "v");
    let key_bytes = {
        // Mirror `interned_key_bytes` in eval_bytes.rs: bin-encoded
        // variable-width interned key.
        let id = key.id();
        if id <= u8::MAX as u64 {
            vec![0xc4, 1, id as u8]
        } else {
            panic!("test key id unexpectedly large")
        }
    };
    let mut buf = Vec::new();
    buf.push(0x81); // fixmap, 1 entry
    buf.extend_from_slice(&key_bytes);
    buf.push(0xcf); // uint64 marker
    buf.extend_from_slice(&u.to_be_bytes());
    Bytes::from(buf)
}

#[test]
fn cross_path_u64_two_pow_64_boundary() {
    // u64::MAX vs the f64 value 2^64 (out of u64's range -- u64::MAX ==
    // 2^64 - 1). Mirrors the i64/2^63 case above: a naive
    // `f <= u64::MAX as f64` clamp would wrongly pass here because
    // `u64::MAX as f64` itself rounds UP to exactly `2^64.0`.
    let interner = Interner::new();
    let bytes = encode_u64_field_record(&interner, u64::MAX);
    let two_pow_64 = 18446744073709551616.0_f64; // 2^64, exact f64 literal

    let lt_filter = Filter::Lt {
        field: vec!["v".to_string()],
        value: FilterValue::Float(two_pow_64),
    };
    let compiled = compile_filter(&lt_filter, &interner);
    let bytes_result = compiled.matches_msgpack_bytes(&bytes);
    assert_eq!(
        bytes_result,
        Some(true),
        "u64::MAX must be exactly Lt 2^64 via the bytes pre-filter"
    );

    let eq_filter = Filter::Eq {
        field: vec!["v".to_string()],
        value: FilterValue::Float(two_pow_64),
    };
    let compiled_eq = compile_filter(&eq_filter, &interner);
    assert_eq!(
        compiled_eq.matches_msgpack_bytes(&bytes),
        Some(false),
        "u64::MAX must NOT equal 2^64 via the bytes pre-filter"
    );
}

#[test]
fn cross_path_u64_exact_equal_large_value() {
    // A u64 value at/above 2^63 (would not fit i64) that IS exactly
    // representable as f64 (a power of two): 2^63 itself.
    let interner = Interner::new();
    let large: u64 = 1u64 << 63;
    let bytes = encode_u64_field_record(&interner, large);

    let eq_filter = Filter::Eq {
        field: vec!["v".to_string()],
        value: FilterValue::Float(large as f64),
    };
    let compiled = compile_filter(&eq_filter, &interner);
    assert_eq!(
        compiled.matches_msgpack_bytes(&bytes),
        Some(true),
        "u64 2^63 must equal its exact f64 representation via the bytes pre-filter"
    );
}
