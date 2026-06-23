//! [`SchemaValidator`] — declarative `impl RecordValidator`.
//!
//! Compiles a set of [`FieldRule`]s into a validator that checks every rule
//! against the incoming record via `&dyn RecordFields`.  Pure checks (Phase A/B)
//! plus async DB-read FK checks (Phase C2).

use async_trait::async_trait;

use crate::query::TableRef;
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
        ctx: &ValidatorCtx<'_>,
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
                    // Null is allowed — FK on NULL is skipped (standard SQL
                    // semantics: a NULL foreign key is always valid).
                }
                Some(shamir_types::record_view::Kind::Null) => {
                    v.field_error(rule.path.clone(), "null_not_allowed");
                }
                // Field present with a value — run type + constraint checks.
                Some(_) => {
                    rule.check_extended(fields, &path_refs, Some(ctx), &mut v);

                    // Phase C2 — foreign_key existence check (async DB read).
                    // Runs only when ctx.db() is Some (tx-mode write path with
                    // a resolver); silently skipped under autocommit / unit
                    // tests where no resolver is wired (fail-open, matching
                    // the scalar-bridge precedent).
                    if let Some(fk) = &rule.constraints.foreign_key {
                        if let Some(db) = ctx.db() {
                            // Materialise the field value as QueryValue for the
                            // FK lookup.
                            if let Some(field_qv) = rule.materialize_as_qv(fields, &path_refs) {
                                let table_ref = TableRef::new(&fk.ref_table);
                                match db.exists_in(&table_ref, &fk.ref_field, &field_qv).await {
                                    Ok(true) => {
                                        // Referenced value exists — FK satisfied.
                                    }
                                    Ok(false) => {
                                        v.field_error(rule.path.clone(), "fk_violation");
                                    }
                                    Err(e) => {
                                        // DB read error — fail closed.
                                        log::warn!(
                                            "SchemaValidator FK check error on field {:?}: {}",
                                            rule.path,
                                            e
                                        );
                                        v.field_error(rule.path.clone(), "fk_violation");
                                    }
                                }
                            }
                        }
                        // else: ctx.db() == None → FK check silently skipped.
                        // Batch-consistency is only guaranteed in tx-mode
                        // (multi-statement transactions where the resolver is
                        // wired). Autocommit single-op inserts skip FK because
                        // the implicit-tx path does not wire a cross-table
                        // resolver.
                    }

                    // Phase C3 — unique constraint (async DB read).
                    // Runs only when ctx.db() is Some (tx-mode write path);
                    // silently skipped under autocommit (ctx.db() == None) —
                    // same fail-open precedent as FK.
                    if rule.constraints.unique {
                        if let Some(db) = ctx.db() {
                            if let Some(field_qv) = rule.materialize_as_qv(fields, &path_refs) {
                                // On UPDATE: if the unique field's value has not
                                // changed, skip the check — the record already
                                // owns that value in committed state.
                                let old_value = _old.and_then(|old_fields| {
                                    rule.materialize_as_qv(old_fields, &path_refs)
                                });
                                let skip = old_value
                                    .as_ref()
                                    .map(|ov| ov == &field_qv)
                                    .unwrap_or(false);

                                if !skip {
                                    let field_name = &rule.path[0];
                                    // exclude_rid: None for INSERT (no old record);
                                    // for UPDATE the old_value != new_value case means
                                    // the committed record still holds the old value,
                                    // so exists_in_self for the NEW value won't
                                    // self-match.
                                    match db.exists_in_self(field_name, &field_qv, None).await {
                                        Ok(true) => {
                                            v.field_error(rule.path.clone(), "unique_violation");
                                        }
                                        Ok(false) => {
                                            // No duplicate — unique satisfied.
                                        }
                                        Err(e) => {
                                            // DB read error — fail closed.
                                            log::warn!(
                                                "SchemaValidator unique check error \
                                                 on field {:?}: {}",
                                                rule.path,
                                                e
                                            );
                                            v.field_error(rule.path.clone(), "unique_violation");
                                        }
                                    }
                                }
                            }
                        }
                        // else: ctx.db() == None → unique check silently
                        // skipped. Autocommit (implicit tx) does not wire
                        // the resolver / ValidatorDb, so relational checks
                        // (FK, unique) are not enforced — matching the FK
                        // precedent.
                    }
                }
            }
        }

        v
    }
}
