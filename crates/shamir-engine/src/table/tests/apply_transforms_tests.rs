//! Unit tests for `apply_transforms` (③.2b/③.2d transform framework).
//!
//! Tests call `apply_transforms` directly with hand-built specs and a fixed
//! `now_ns` so results are deterministic without any I/O or storage.

use shamir_query_types::filter::FilterValue;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::function::builtin_scalars;
use crate::table::write_helpers::apply_transforms;
use crate::validator::TransformSpec;

// Fixed timestamp used across all tests for determinism.
const NOW_NS: u64 = 1_700_000_000_000_000_000_u64;

// ── helpers ──────────────────────────────────────────────────────────────────

fn map_with(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_string(), v.clone());
    }
    QueryValue::Map(m)
}

fn get_int(rec: &QueryValue, field: &str) -> Option<i64> {
    match rec {
        QueryValue::Map(m) => match m.get(field) {
            Some(QueryValue::Int(i)) => Some(*i),
            _ => None,
        },
        _ => None,
    }
}

fn get_str(rec: &QueryValue, field: &str) -> Option<String> {
    match rec {
        QueryValue::Map(m) => match m.get(field) {
            Some(QueryValue::Str(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn has_key(rec: &QueryValue, field: &str) -> bool {
    match rec {
        QueryValue::Map(m) => m.contains_key(field),
        _ => false,
    }
}

fn spec(field: &str, s: TransformSpec) -> (Vec<String>, TransformSpec) {
    (vec![field.to_string()], s)
}

// ── AutoNow tests ─────────────────────────────────────────────────────────────

#[test]
fn auto_now_overwrites_existing_value() {
    let mut rec = map_with(&[("updated_at", QueryValue::Int(999))]);
    let transforms = vec![spec("updated_at", TransformSpec::AutoNow)];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    assert_eq!(get_int(&rec, "updated_at"), Some(NOW_NS as i64));
}

#[test]
fn auto_now_stamps_absent_field() {
    let mut rec = map_with(&[("name", QueryValue::Str("alice".into()))]);
    let transforms = vec![spec("updated_at", TransformSpec::AutoNow)];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    assert_eq!(get_int(&rec, "updated_at"), Some(NOW_NS as i64));
}

/// ③.2d — AutoNow fires on UPDATE (is_insert=false) too.
#[test]
fn auto_now_fires_on_update_path() {
    let mut rec = map_with(&[("name", QueryValue::Str("alice".into()))]);
    let transforms = vec![spec("updated_at", TransformSpec::AutoNow)];

    // is_insert=false — the UPDATE path.
    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, false);

    // AutoNow is unconditional: fires even on UPDATE.
    assert_eq!(get_int(&rec, "updated_at"), Some(NOW_NS as i64));
}

/// ③.2d — AutoNow overwrites on UPDATE path too.
#[test]
fn auto_now_overwrites_on_update_path() {
    let mut rec = map_with(&[("updated_at", QueryValue::Int(42))]);
    let transforms = vec![spec("updated_at", TransformSpec::AutoNow)];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, false);

    assert_eq!(get_int(&rec, "updated_at"), Some(NOW_NS as i64));
}

// ── AutoNowAdd tests ──────────────────────────────────────────────────────────

#[test]
fn auto_now_add_stamps_absent_field() {
    let mut rec = map_with(&[("name", QueryValue::Str("bob".into()))]);
    let transforms = vec![spec("created_at", TransformSpec::AutoNowAdd)];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    assert_eq!(get_int(&rec, "created_at"), Some(NOW_NS as i64));
}

#[test]
fn auto_now_add_does_not_overwrite_present_value() {
    let original_ts: i64 = 42_000_000;
    let mut rec = map_with(&[("created_at", QueryValue::Int(original_ts))]);
    let transforms = vec![spec("created_at", TransformSpec::AutoNowAdd)];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    // Must remain unchanged — an explicitly-supplied value is preserved.
    assert_eq!(get_int(&rec, "created_at"), Some(original_ts));
}

#[test]
fn auto_now_add_does_not_overwrite_explicit_null() {
    let mut rec = map_with(&[("created_at", QueryValue::Null)]);
    let transforms = vec![spec("created_at", TransformSpec::AutoNowAdd)];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    // Explicit Null is present → absence check fails → no stamp.
    assert!(has_key(&rec, "created_at"));
    assert!(matches!(
        rec,
        QueryValue::Map(ref m) if m.get("created_at") == Some(&QueryValue::Null)
    ));
}

/// ③.2d — AutoNowAdd is SKIPPED on UPDATE path (is_insert=false).
#[test]
fn auto_now_add_skipped_on_update_path() {
    // Absent field — would be stamped on INSERT, must NOT be stamped on UPDATE.
    let mut rec = map_with(&[("name", QueryValue::Str("carol".into()))]);
    let transforms = vec![spec("created_at", TransformSpec::AutoNowAdd)];

    // is_insert=false — the UPDATE path (partial set-map, no created_at present).
    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, false);

    // AutoNowAdd must NOT fire on UPDATE — created_at stays absent.
    assert!(!has_key(&rec, "created_at"));
}

// ── ComputedDefault tests ─────────────────────────────────────────────────────

#[test]
fn computed_default_literal_stamps_absent_field() {
    let mut rec = map_with(&[("name", QueryValue::Str("carol".into()))]);
    let transforms = vec![spec(
        "role",
        TransformSpec::ComputedDefault(FilterValue::String("user".to_string())),
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    assert_eq!(get_str(&rec, "role"), Some("user".to_string()));
}

#[test]
fn computed_default_does_not_overwrite_present_value() {
    let mut rec = map_with(&[("role", QueryValue::Str("admin".into()))]);
    let transforms = vec![spec(
        "role",
        TransformSpec::ComputedDefault(FilterValue::String("user".to_string())),
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    // Present value must be preserved.
    assert_eq!(get_str(&rec, "role"), Some("admin".to_string()));
}

#[test]
fn computed_default_int_literal() {
    let mut rec = map_with(&[("name", QueryValue::Str("dave".into()))]);
    let transforms = vec![spec(
        "score",
        TransformSpec::ComputedDefault(FilterValue::Int(0)),
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    assert_eq!(get_int(&rec, "score"), Some(0));
}

/// ③.2d — ComputedDefault is SKIPPED on UPDATE path (is_insert=false).
#[test]
fn computed_default_skipped_on_update_path() {
    let mut rec = map_with(&[("name", QueryValue::Str("dave".into()))]);
    let transforms = vec![spec(
        "score",
        TransformSpec::ComputedDefault(FilterValue::Int(0)),
    )];

    // is_insert=false — the UPDATE path.
    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, false);

    // ComputedDefault must NOT fire on UPDATE.
    assert!(!has_key(&rec, "score"));
}

// ── Non-map record ────────────────────────────────────────────────────────────

#[test]
fn non_map_record_is_noop() {
    let mut rec = QueryValue::Str("not a map".into());
    let transforms = vec![spec("field", TransformSpec::AutoNow)];
    let before = rec.clone();

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    assert_eq!(rec, before);
}

// ── Empty transforms list ─────────────────────────────────────────────────────

#[test]
fn empty_transforms_is_noop() {
    let mut rec = map_with(&[("x", QueryValue::Int(1))]);
    let before = rec.clone();

    apply_transforms(&mut rec, &[], builtin_scalars(), NOW_NS, true);

    assert_eq!(rec, before);
}

// ── Multi-segment path skipped ────────────────────────────────────────────────

#[test]
fn multi_segment_path_is_skipped() {
    let mut rec = map_with(&[("a", QueryValue::Int(1))]);
    let transforms = vec![(
        vec!["address".to_string(), "zip".to_string()],
        TransformSpec::AutoNow,
    )];

    apply_transforms(&mut rec, &transforms, builtin_scalars(), NOW_NS, true);

    // Multi-segment: skipped — no "address" key inserted.
    assert!(!has_key(&rec, "address"));
}
