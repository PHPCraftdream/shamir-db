//! Unit tests for Phase B declarative-schema checks:
//! scalar-bridge, format, cross-field.
//!
//! Each check has an accept + reject case; the scalar-bridge `None`-resolver
//! path (silent skip) is covered explicitly.

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::validator::encode::Validation;
use crate::validator::record_fields::OwnedFields;
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::cross_field::{CompareOp, CrossFieldCompare};
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::format::FormatKind;
use crate::validator::schema::type_tag::TypeTag;

// ── helpers ─────────────────────────────────────────────────────────────────

fn fields_from(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    QueryValue::Map(m)
}

fn check_extended_no_ctx(rule: &FieldRule, qv: &QueryValue) -> Validation {
    let fields = OwnedFields { qv };
    let path_refs: Vec<&str> = rule.path.iter().map(String::as_str).collect();
    let mut v = Validation::accept();
    rule.check_extended(&fields, &path_refs, None, &mut v);
    v
}

fn check_extended_with_resolver(
    rule: &FieldRule,
    qv: &QueryValue,
    resolver: &ScalarResolver,
) -> Validation {
    use shamir_types::access::Actor;
    use shamir_types::core::interner::Interner;

    let fields = OwnedFields { qv };
    let path_refs: Vec<&str> = rule.path.iter().map(String::as_str).collect();
    let interner = Interner::new();
    let actor = Actor::default();
    let ctx =
        crate::validator::record_validator::ValidatorCtx::with_scalars(&actor, &interner, resolver);
    let mut v = Validation::accept();
    rule.check_extended(&fields, &path_refs, Some(&ctx), &mut v);
    v
}

fn rule_with(path: &str, ty: TypeTag, constraints: Constraints) -> FieldRule {
    FieldRule {
        path: vec![path.to_string()],
        ty,
        constraints,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 1. SCALAR-BRIDGE
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn scalar_bridge_accepts_valid_email() {
    let rule = rule_with(
        "email",
        TypeTag::String,
        Constraints {
            scalar: Some("validate/is_email".into()),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("email", QueryValue::Str("alice@example.com".into()))]);
    let resolver = ScalarResolver::builtins_only();
    let v = check_extended_with_resolver(&rule, &qv, &resolver);
    assert!(v.is_ok(), "valid email should pass: {:?}", v.errors);
}

#[test]
fn scalar_bridge_rejects_invalid_email() {
    let rule = rule_with(
        "email",
        TypeTag::String,
        Constraints {
            scalar: Some("validate/is_email".into()),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("email", QueryValue::Str("not-an-email".into()))]);
    let resolver = ScalarResolver::builtins_only();
    let v = check_extended_with_resolver(&rule, &qv, &resolver);
    assert!(!v.is_ok(), "invalid email should fail");
    assert_eq!(v.errors[0].code, "scalar_rejected");
}

#[test]
fn scalar_bridge_none_resolver_skips_silently() {
    // scalar-bridge rule but no resolver wired — must NOT panic, must NOT
    // produce an error (silent skip).
    let rule = rule_with(
        "email",
        TypeTag::String,
        Constraints {
            scalar: Some("validate/is_email".into()),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("email", QueryValue::Str("not-an-email".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(
        v.is_ok(),
        "scalar-bridge with None resolver should skip: {:?}",
        v.errors
    );
}

#[test]
fn scalar_bridge_unknown_function_fails_closed() {
    let rule = rule_with(
        "x",
        TypeTag::String,
        Constraints {
            scalar: Some("does_not_exist".into()),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("x", QueryValue::Str("anything".into()))]);
    let resolver = ScalarResolver::builtins_only();
    let v = check_extended_with_resolver(&rule, &qv, &resolver);
    assert!(!v.is_ok(), "unknown scalar should fail closed");
    assert_eq!(v.errors[0].code, "scalar_check_failed");
}

// ═══════════════════════════════════════════════════════════════════════
// 2. FORMAT
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn format_email_accepts_valid() {
    let rule = rule_with(
        "email",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Email),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("email", QueryValue::Str("bob@example.com".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "valid email format should pass: {:?}", v.errors);
}

#[test]
fn format_email_rejects_invalid() {
    let rule = rule_with(
        "email",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Email),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("email", QueryValue::Str("no-at-sign".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok(), "invalid email format should fail");
    assert_eq!(v.errors[0].code, "bad_format");
}

#[test]
fn format_url_accepts_valid() {
    let rule = rule_with(
        "homepage",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Url),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![(
        "homepage",
        QueryValue::Str("https://example.com/page".into()),
    )]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "valid url should pass: {:?}", v.errors);
}

#[test]
fn format_url_rejects_invalid() {
    let rule = rule_with(
        "homepage",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Url),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("homepage", QueryValue::Str("not-a-url".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok());
    assert_eq!(v.errors[0].code, "bad_format");
}

#[test]
fn format_uuid_accepts_valid() {
    let rule = rule_with(
        "id",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Uuid),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![(
        "id",
        QueryValue::Str("550e8400-e29b-41d4-a716-446655440000".into()),
    )]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "valid uuid should pass: {:?}", v.errors);
}

#[test]
fn format_uuid_rejects_invalid() {
    let rule = rule_with(
        "id",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Uuid),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("id", QueryValue::Str("not-a-uuid".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok());
    assert_eq!(v.errors[0].code, "bad_format");
}

#[test]
fn format_date_accepts_rfc3339() {
    let rule = rule_with(
        "created_at",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Date),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![(
        "created_at",
        QueryValue::Str("2024-01-31T08:30:00Z".into()),
    )]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "rfc3339 date should pass: {:?}", v.errors);
}

#[test]
fn format_date_accepts_bare_calendar_date() {
    let rule = rule_with(
        "dob",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Date),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("dob", QueryValue::Str("2024-02-29".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "2024-02-29 is a leap-year date: {:?}", v.errors);
}

#[test]
fn format_date_rejects_invalid_calendar_date() {
    let rule = rule_with(
        "dob",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Date),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("dob", QueryValue::Str("2023-02-29".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok(), "2023-02-29 is NOT a leap-year date");
    assert_eq!(v.errors[0].code, "bad_format");
}

#[test]
fn format_date_rejects_garbage() {
    let rule = rule_with(
        "dob",
        TypeTag::String,
        Constraints {
            format: Some(FormatKind::Date),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("dob", QueryValue::Str("hello".into()))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok());
    assert_eq!(v.errors[0].code, "bad_format");
}

#[test]
fn format_on_non_string_records_type_mismatch() {
    let rule = rule_with(
        "email",
        TypeTag::Any,
        Constraints {
            format: Some(FormatKind::Email),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("email", QueryValue::Int(42))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok());
    assert_eq!(v.errors[0].code, "format_type_mismatch");
}

// ═══════════════════════════════════════════════════════════════════════
// 3. CROSS-FIELD
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn cross_field_le_accepts_when_start_le_end() {
    let rule = rule_with(
        "start",
        TypeTag::Int,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["end".into()], CompareOp::Le)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![
        ("start", QueryValue::Int(10)),
        ("end", QueryValue::Int(20)),
    ]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "start <= end should pass: {:?}", v.errors);
}

#[test]
fn cross_field_le_rejects_when_start_gt_end() {
    let rule = rule_with(
        "start",
        TypeTag::Int,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["end".into()], CompareOp::Le)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![
        ("start", QueryValue::Int(30)),
        ("end", QueryValue::Int(20)),
    ]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok(), "start > end should fail");
    assert_eq!(v.errors[0].code, "compare_violation");
}

#[test]
fn cross_field_lt_accepts() {
    let rule = rule_with(
        "lo",
        TypeTag::Int,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["hi".into()], CompareOp::Lt)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("lo", QueryValue::Int(1)), ("hi", QueryValue::Int(2))]);
    assert!(check_extended_no_ctx(&rule, &qv).is_ok());
}

#[test]
fn cross_field_lt_rejects_equal() {
    let rule = rule_with(
        "lo",
        TypeTag::Int,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["hi".into()], CompareOp::Lt)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("lo", QueryValue::Int(5)), ("hi", QueryValue::Int(5))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok(), "lo == hi should fail Lt");
    assert_eq!(v.errors[0].code, "compare_violation");
}

#[test]
fn cross_field_skipped_when_other_absent() {
    // `end` is absent — the check is silently skipped.
    let rule = rule_with(
        "start",
        TypeTag::Int,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["end".into()], CompareOp::Le)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![("start", QueryValue::Int(10))]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(v.is_ok(), "absent other path should skip: {:?}", v.errors);
}

#[test]
fn cross_field_type_mismatch_on_incomparable_types() {
    // Int vs Str → incomparable → compare_type_mismatch.
    let rule = rule_with(
        "start",
        TypeTag::Int,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["end".into()], CompareOp::Le)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![
        ("start", QueryValue::Int(10)),
        ("end", QueryValue::Str("hello".into())),
    ]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok(), "incomparable types should fail");
    assert_eq!(v.errors[0].code, "compare_type_mismatch");
}

#[test]
fn cross_field_string_eq_accepts() {
    let rule = rule_with(
        "a",
        TypeTag::String,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["b".into()], CompareOp::Eq)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![
        ("a", QueryValue::Str("x".into())),
        ("b", QueryValue::Str("x".into())),
    ]);
    assert!(check_extended_no_ctx(&rule, &qv).is_ok());
}

#[test]
fn cross_field_string_eq_rejects() {
    let rule = rule_with(
        "a",
        TypeTag::String,
        Constraints {
            compare: Some(CrossFieldCompare::new(vec!["b".into()], CompareOp::Eq)),
            ..Default::default()
        },
    );
    let qv = fields_from(vec![
        ("a", QueryValue::Str("x".into())),
        ("b", QueryValue::Str("y".into())),
    ]);
    let v = check_extended_no_ctx(&rule, &qv);
    assert!(!v.is_ok(), "different strings should fail Eq");
    assert_eq!(v.errors[0].code, "compare_violation");
}
