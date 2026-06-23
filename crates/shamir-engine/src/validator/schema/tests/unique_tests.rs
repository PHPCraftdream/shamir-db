//! Unit tests for Phase C3 unique declarative constraint.
//!
//! These are pure unit tests that exercise the SchemaValidator's unique check
//! path.  The unique check requires ctx.db() == Some to fire; when
//! ctx.db() == None, unique is silently skipped (same precedent as FK).

use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::validator::encode::Validation;
use crate::validator::record_fields::OwnedFields;
use crate::validator::record_validator::{RecordValidator, ValidatorCtx};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
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

fn rule_with_unique(path: &str, ty: TypeTag) -> FieldRule {
    FieldRule {
        path: vec![path.to_string()],
        ty,
        constraints: Constraints {
            unique: true,
            ..Default::default()
        },
    }
}

/// Run the SchemaValidator with no ctx.db() (unique silently skipped).
async fn validate_no_db(validator: &SchemaValidator, qv: &QueryValue) -> Validation {
    let fields = OwnedFields { qv };
    let interner = Interner::new();
    let actor = Actor::default();
    let ctx = ValidatorCtx::new(&actor, &interner);
    validator
        .validate(
            Some(&fields as &dyn crate::validator::record_fields::RecordFields),
            None,
            &ctx,
        )
        .await
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Unique without ctx.db() — silently skipped
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unique_no_db_context_skips_silently() {
    let rule = rule_with_unique("email", TypeTag::String);
    let sv = SchemaValidator::new(vec![rule]);
    let qv = fields_from(vec![("email", QueryValue::Str("a@b.com".into()))]);
    let v = validate_no_db(&sv, &qv).await;
    assert!(
        v.is_ok(),
        "unique with no db context should skip silently: {:?}",
        v.errors
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Unique on NULL value — silently skipped (standard SQL semantics)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unique_null_value_skips() {
    let rule = FieldRule {
        path: vec!["email".to_string()],
        ty: TypeTag::String,
        constraints: Constraints {
            unique: true,
            nullable: true,
            ..Default::default()
        },
    };
    let sv = SchemaValidator::new(vec![rule]);
    let qv = fields_from(vec![("email", QueryValue::Null)]);
    let v = validate_no_db(&sv, &qv).await;
    assert!(
        v.is_ok(),
        "unique on NULL value should skip: {:?}",
        v.errors
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 3. Constraints struct — unique defaults to false
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn unique_defaults_to_false() {
    let c = Constraints::default();
    assert!(!c.unique, "unique should default to false");
}
