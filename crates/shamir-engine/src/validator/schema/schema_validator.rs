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

    /// Collect all foreign-key references declared in the rules.
    ///
    /// Returns `(field_path, fk_ref)` for every rule that has a
    /// `constraints.foreign_key` set. Used by Phase D reverse-FK
    /// discovery (the RESTRICT gate).
    pub fn collect_fk_refs(&self) -> Vec<(Vec<String>, super::ForeignKeyRef)> {
        self.rules
            .iter()
            .filter_map(|r| {
                r.constraints
                    .foreign_key
                    .as_ref()
                    .map(|fk| (r.path.clone(), fk.clone()))
            })
            .collect()
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
                    // Runs whenever ctx.db() is Some AND a cross-table resolver
                    // is wired. In production the server wraps every batch in a
                    // tx context with the resolver, so FK fires on BOTH
                    // transactional and autocommit writes (see the
                    // `autocommit also enforces FK` e2e). It is skipped only on
                    // the raw-engine implicit path that passes resolver = None
                    // (query_runner autocommit arm / unit fixtures) — there
                    // db.exists_in returns Ok(false) → fail-open skip.
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
                        // else: ctx.db() == None → FK check skipped. This is
                        // the raw-engine path with no tx threaded (legacy/unit
                        // callers). Through the server, ctx.db() is always Some
                        // and FK is enforced; only the resolver=None implicit
                        // arm fails open (see foreign_key doc on Constraints).
                    }

                    // Phase C3 — unique constraint (async DB read).
                    // Runs whenever ctx.db() is Some. The unique probe
                    // (exists_in_self) reads SELF-table state and needs NO
                    // cross-table resolver, so it is enforced on BOTH
                    // transactional and autocommit writes (the autocommit path
                    // routes through an implicit tx → ctx.db() is Some). See
                    // the `autocommit also enforces unique` e2e. Skipped only
                    // when no tx is threaded at all (raw-engine/unit callers).
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
                        // else: ctx.db() == None → unique check skipped. This
                        // only happens on the raw-engine path with no tx
                        // threaded (legacy/unit callers). The autocommit path
                        // DOES thread an implicit tx, so ctx.db() is Some and
                        // unique IS enforced there (no resolver needed — the
                        // probe is self-table). See the `autocommit also
                        // enforces unique` e2e.
                    }
                }
            }
        }

        v
    }

    fn fk_refs(&self) -> Vec<(Vec<String>, super::ForeignKeyRef)> {
        self.collect_fk_refs()
    }

    fn nullable_for_field(&self, field: &str) -> Option<bool> {
        // Match single-segment fields directly; for multi-segment paths,
        // compare the dot-joined form.
        self.rules.iter().find_map(|r| {
            let matches = r.path.len() == 1 && r.path[0] == field;
            if matches {
                Some(r.constraints.nullable)
            } else {
                None
            }
        })
    }
}
