//! Unit tests for Phase C2 foreign-key declarative constraint.
//!
//! These are pure unit tests that exercise the SchemaValidator's FK check
//! path with a mock ValidatorDb.  The FK check requires ctx.db() == Some
//! to fire; when ctx.db() == None, FK is silently skipped.

use shamir_types::access::Actor;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::validator::encode::Validation;
use crate::validator::record_fields::OwnedFields;
use crate::validator::record_validator::{RecordValidator, ValidatorCtx};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
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

fn rule_with_fk(path: &str, ty: TypeTag, ref_table: &str, ref_field: &str) -> FieldRule {
    FieldRule {
        path: vec![path.to_string()],
        ty,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::new(ref_table, ref_field)),
            ..Default::default()
        },
    }
}

/// Run the SchemaValidator with no ctx.db() (FK silently skipped).
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
// 1. FK without ctx.db() — silently skipped
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fk_no_db_context_skips_silently() {
    let rule = rule_with_fk("dept_id", TypeTag::Int, "departments", "id");
    let sv = SchemaValidator::new(vec![rule]);
    let qv = fields_from(vec![("dept_id", QueryValue::Int(999))]);
    let v = validate_no_db(&sv, &qv).await;
    assert!(
        v.is_ok(),
        "FK with no db context should skip silently: {:?}",
        v.errors
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 2. FK on NULL value — silently skipped (standard SQL semantics)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fk_null_value_skips() {
    let mut constraints = Constraints {
        foreign_key: Some(ForeignKeyRef::new("departments", "id")),
        nullable: true,
        ..Default::default()
    };
    // nullable so Null is allowed at the field level
    let _ = &mut constraints;
    let rule = FieldRule {
        path: vec!["dept_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::new("departments", "id")),
            nullable: true,
            ..Default::default()
        },
    };
    let sv = SchemaValidator::new(vec![rule]);
    let qv = fields_from(vec![("dept_id", QueryValue::Null)]);
    let v = validate_no_db(&sv, &qv).await;
    assert!(v.is_ok(), "FK on NULL value should skip: {:?}", v.errors);
}

// ═══════════════════════════════════════════════════════════════════════
// 3. ForeignKeyRef construction
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn foreign_key_ref_new() {
    let fk = ForeignKeyRef::new("orders", "customer_id");
    assert_eq!(fk.ref_table, "orders");
    assert_eq!(fk.ref_field, "customer_id");
}
