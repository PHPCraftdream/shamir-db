//! Integration tests for [`SchemaValidator`] as a [`RecordValidator`].
//!
//! Tests the full `validate()` path: required/nullable gating + type checks
//! + constraint checks + error accumulation + DELETE passthrough.

use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::validator::encode::Validation;
use crate::validator::record_fields::OwnedFields;
use crate::validator::record_validator::{RecordValidator, ValidatorCtx};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::rule_builder::rule;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;

// ── helpers ─────────────────────────────────────────────────────────────────

fn fields_from(pairs: Vec<(&str, QueryValue)>) -> QueryValue {
    let mut m = new_map();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    QueryValue::Map(m)
}

async fn run_schema(rules: Vec<FieldRule>, new_record: Option<&QueryValue>) -> Validation {
    let sv = SchemaValidator::new(rules);
    let interner = Interner::new();
    let actor = Actor::System;
    let ctx = ValidatorCtx::new(&actor, &interner);

    match new_record {
        Some(qv) => {
            let fields = OwnedFields { qv };
            sv.validate(Some(&fields), None, &ctx).await
        }
        None => sv.validate(None, None, &ctx).await,
    }
}

// ── Empty schema ────────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_schema_accepts_anything() {
    let qv = fields_from(vec![("x", QueryValue::Int(42))]);
    let v = run_schema(vec![], Some(&qv)).await;
    assert!(v.is_ok());
}

// ── Required / nullable ─────────────────────────────────────────────────────

#[tokio::test]
async fn required_field_missing() {
    let rules = vec![rule(["email"]).string().required().build()];
    let qv = fields_from(vec![("name", QueryValue::Str("alice".into()))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert!(!v.is_ok());
    assert_eq!(v.errors.len(), 1);
    assert_eq!(v.errors[0].code, "missing_required");
    assert_eq!(
        v.errors[0].field.as_ref().unwrap(),
        &vec!["email".to_string()]
    );
}

#[tokio::test]
async fn optional_field_missing_is_ok() {
    let rules = vec![rule(["email"]).string().build()];
    let qv = fields_from(vec![("name", QueryValue::Str("alice".into()))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert!(v.is_ok());
}

#[tokio::test]
async fn null_field_rejected_when_not_nullable() {
    let rules = vec![rule(["email"]).string().required().build()];
    let qv = fields_from(vec![("email", QueryValue::Null)]);
    let v = run_schema(rules, Some(&qv)).await;
    assert!(!v.is_ok());
    assert_eq!(v.errors[0].code, "null_not_allowed");
}

#[tokio::test]
async fn null_field_accepted_when_nullable() {
    let rules = vec![rule(["email"]).string().required().nullable().build()];
    let qv = fields_from(vec![("email", QueryValue::Null)]);
    let v = run_schema(rules, Some(&qv)).await;
    assert!(v.is_ok());
}

// ── DELETE passthrough ──────────────────────────────────────────────────────

#[tokio::test]
async fn delete_passthrough() {
    let rules = vec![rule(["email"]).string().required().build()];
    let v = run_schema(rules, None).await;
    assert!(v.is_ok());
}

// ── Error accumulation ──────────────────────────────────────────────────────

#[tokio::test]
async fn multiple_errors_accumulated() {
    let rules = vec![
        rule(["email"]).string().required().build(),
        rule(["age"]).int().required().build(),
        rule(["name"]).string().required().build(),
    ];
    // All three fields missing.
    let qv = fields_from(vec![]);
    let v = run_schema(rules, Some(&qv)).await;
    assert_eq!(v.errors.len(), 3);
    assert!(v.errors.iter().all(|e| e.code == "missing_required"));
}

#[tokio::test]
async fn mixed_errors() {
    let rules = vec![
        rule(["email"]).string().required().build(),
        rule(["age"]).int().min(0).max(150).build(),
    ];
    let qv = fields_from(vec![("age", QueryValue::Int(200))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert_eq!(v.errors.len(), 2);

    let codes: Vec<&str> = v.errors.iter().map(|e| e.code.as_str()).collect();
    assert!(codes.contains(&"missing_required"));
    assert!(codes.contains(&"out_of_range"));
}

// ── Type + constraint combos ────────────────────────────────────────────────

#[tokio::test]
async fn int_unsigned_via_builder() {
    let rules = vec![rule(["count"]).int().unsigned().build()];
    let qv = fields_from(vec![("count", QueryValue::Int(-1))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert_eq!(v.errors[0].code, "out_of_range");
}

#[tokio::test]
async fn string_max_len_via_builder() {
    let rules = vec![rule(["name"]).string().max_len(5).build()];
    let qv = fields_from(vec![("name", QueryValue::Str("toolongname".into()))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert_eq!(v.errors[0].code, "too_long");
}

#[tokio::test]
async fn nested_path_check() {
    let rules = vec![rule(["address", "zip"]).string().len(5).required().build()];
    let mut addr = new_map();
    addr.insert("zip".to_string(), QueryValue::Str("123".into()));
    let qv = fields_from(vec![("address", QueryValue::Map(addr))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert_eq!(v.errors[0].code, "wrong_length");
}

#[tokio::test]
async fn one_of_via_schema() {
    let rules = vec![FieldRule {
        path: vec!["status".into()],
        ty: TypeTag::String,
        constraints: Constraints {
            one_of: Some(vec![
                QueryValue::Str("active".into()),
                QueryValue::Str("inactive".into()),
            ]),
            ..Default::default()
        },
    }];
    let qv = fields_from(vec![("status", QueryValue::Str("deleted".into()))]);
    let v = run_schema(rules, Some(&qv)).await;
    assert_eq!(v.errors[0].code, "not_in_enum");
}

#[tokio::test]
async fn full_schema_accept() {
    let rules = vec![
        rule(["email"]).string().max_len(255).required().build(),
        rule(["age"]).int().min(0).max(150).build(),
        rule(["address", "zip"]).string().len(5).build(),
    ];

    let mut addr = new_map();
    addr.insert("zip".to_string(), QueryValue::Str("12345".into()));

    let qv = fields_from(vec![
        ("email", QueryValue::Str("alice@example.com".into())),
        ("age", QueryValue::Int(30)),
        ("address", QueryValue::Map(addr)),
    ]);

    let v = run_schema(rules, Some(&qv)).await;
    assert!(v.is_ok());
}
