//! [`SchemaValidator`] — declarative `impl RecordValidator`.
//!
//! Compiles a set of [`FieldRule`]s into a validator that checks every rule
//! against the incoming record via `&dyn RecordFields`.  Pure, in-process,
//! no DB access.

use async_trait::async_trait;

use crate::validator::encode::Validation;
use crate::validator::record_fields::RecordFields;
use crate::validator::record_validator::{RecordValidator, ValidatorCtx};

use super::field_rule::FieldRule;

/// Declarative schema validator.
///
/// Holds a compiled rule-set and implements [`RecordValidator`].  The
/// validator is pure: it reads fields via `RecordFields` and never touches
/// the database.  Registration and auto-binding are handled by a higher
/// layer (Phase A2).
#[derive(Debug, Clone)]
pub struct SchemaValidator {
    /// Compiled field rules.
    pub rules: Vec<FieldRule>,
}

impl SchemaValidator {
    /// Create a new schema validator from a list of field rules.
    pub fn new(rules: Vec<FieldRule>) -> Self {
        Self { rules }
    }
}

#[async_trait]
impl RecordValidator for SchemaValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        // If there is no new record (DELETE), the schema validator accepts
        // unconditionally — field constraints apply only to writes that
        // produce a new record.
        let fields = match new {
            Some(f) => f,
            None => return Validation::accept(),
        };

        let mut v = Validation::accept();

        for rule in &self.rules {
            let path_refs: Vec<&str> = rule.path.iter().map(String::as_str).collect();

            match fields.present(&path_refs) {
                // Field absent.
                None if rule.constraints.required => {
                    v.field_error(rule.path.clone(), "missing_required");
                }
                None => {
                    // Absent and not required — skip.
                }
                // Field present as Null.
                Some(shamir_types::record_view::Kind::Null) if rule.constraints.nullable => {
                    // Null is allowed — skip.
                }
                Some(shamir_types::record_view::Kind::Null) => {
                    v.field_error(rule.path.clone(), "null_not_allowed");
                }
                // Field present with a value — run type + constraint checks.
                Some(_) => {
                    rule.check(fields, &path_refs, &mut v);
                }
            }
        }

        v
    }
}
