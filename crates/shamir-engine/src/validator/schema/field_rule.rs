//! A single field rule and its check logic.
//!
//! [`FieldRule`] combines a field path, a [`TypeTag`], and [`Constraints`].
//! The [`FieldRule::check`] method runs the type assertion and constraint
//! checks against a live `&dyn RecordFields`, accumulating errors into a
//! [`Validation`].

use shamir_types::record_view::{Kind, ScalarRef};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::validator::encode::Validation;
use crate::validator::record_fields::RecordFields;

use super::constraints::{Constraints, Num};
use super::type_tag::TypeTag;

/// A single declarative field rule.
///
/// `path` identifies the field (possibly nested: `["address", "zip"]`).
/// `ty` is the expected type tag.  `constraints` holds numeric / string /
/// collection / enum checks.
#[derive(Debug, Clone)]
pub struct FieldRule {
    /// Dotted field path (each segment is a map key).
    pub path: Vec<String>,
    /// Expected type tag.
    pub ty: TypeTag,
    /// Additional constraints (min/max/len/unsigned/one_of/...).
    pub constraints: Constraints,
}

impl FieldRule {
    /// Run type + constraint checks against a present (non-absent, non-null)
    /// field value.
    ///
    /// Called from `SchemaValidator::validate` after `required` / `nullable`
    /// have already been handled.  The field is guaranteed to be present and
    /// non-null at this point.
    pub fn check(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        // ── Type assertion ──────────────────────────────────────────────
        if !self.type_matches(fields, path) {
            v.field_error(self.path.clone(), "type_mismatch");
            return; // no point checking constraints on wrong type
        }

        // ── Constraints ─────────────────────────────────────────────────
        match self.ty {
            TypeTag::Int => self.check_int(fields, path, v),
            TypeTag::F64 => self.check_f64(fields, path, v),
            TypeTag::String => self.check_string(fields, path, v),
            TypeTag::List => self.check_collection(fields, path, v),
            TypeTag::Map => self.check_collection(fields, path, v),
            TypeTag::Set => self.check_collection(fields, path, v),
            TypeTag::Any => self.check_any(fields, path, v),
            // Bool, Bin, Dec, Null — no numeric/string constraints apply;
            // only `one_of` is relevant, checked below.
            _ => {}
        }

        // one_of applies to any type (it compares materialised values).
        self.check_one_of(fields, path, v);
    }

    // ── Type matching ───────────────────────────────────────────────────

    /// Returns `true` if the runtime value matches the expected [`TypeTag`].
    fn type_matches(&self, fields: &dyn RecordFields, path: &[&str]) -> bool {
        match self.ty {
            TypeTag::Any => true,
            TypeTag::Null => {
                // Null is handled at the SchemaValidator level; if we reach
                // here, the value is present and non-null, so Null tag fails.
                false
            }
            TypeTag::String => fields.str(path).is_some(),
            TypeTag::Int => matches!(fields.scalar(path), Some(ScalarRef::Int(_))),
            TypeTag::F64 => matches!(fields.scalar(path), Some(ScalarRef::F64(_))),
            TypeTag::Bool => matches!(fields.scalar(path), Some(ScalarRef::Bool(_))),
            TypeTag::Bin => matches!(fields.scalar(path), Some(ScalarRef::Bin(_))),
            TypeTag::Dec => {
                // Dec is distinguishable only on OwnedFields (QueryValue::Dec).
                // On ViewFields (lens), Dec collapses to Bin.  We check via
                // materialize: InnerValue::Dec.
                matches!(fields.materialize(path), Some(InnerValue::Dec(_)))
            }
            TypeTag::List => {
                matches!(fields.materialize(path), Some(InnerValue::List(_)))
            }
            TypeTag::Map => {
                // On OwnedFields, `materialize(Map)` returns `InnerValue::Null`
                // because map keys need an interner for InnerValue conversion.
                // Detect Map as: Container kind + NOT List/Set via materialize.
                matches!(fields.present(path), Some(Kind::Container))
                    && !matches!(
                        fields.materialize(path),
                        Some(InnerValue::List(_) | InnerValue::Set(_))
                    )
            }
            TypeTag::Set => {
                matches!(fields.materialize(path), Some(InnerValue::Set(_)))
            }
        }
    }

    // ── Numeric checks ──────────────────────────────────────────────────

    fn check_int(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        let val = match fields.scalar(path) {
            Some(ScalarRef::Int(i)) => i,
            _ => return,
        };

        if self.constraints.unsigned && val < 0 {
            v.field_error(self.path.clone(), "out_of_range");
        }

        if let Some(Num::Int(min)) = self.constraints.min {
            if val < min {
                v.field_error(self.path.clone(), "out_of_range");
            }
        }

        if let Some(Num::Int(max)) = self.constraints.max {
            if val > max {
                v.field_error(self.path.clone(), "out_of_range");
            }
        }
    }

    fn check_f64(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        let val = match fields.scalar(path) {
            Some(ScalarRef::F64(f)) => f,
            _ => return,
        };

        if let Some(Num::F64(min)) = self.constraints.min {
            if val < min {
                v.field_error(self.path.clone(), "out_of_range");
            }
        }

        if let Some(Num::F64(max)) = self.constraints.max {
            if val > max {
                v.field_error(self.path.clone(), "out_of_range");
            }
        }
    }

    // ── String checks ───────────────────────────────────────────────────

    fn check_string(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        let s = match fields.str(path) {
            Some(s) => s,
            None => return,
        };

        let char_len = s.chars().count() as u64;

        if let Some(exact) = self.constraints.len {
            if char_len != exact {
                v.field_error(self.path.clone(), "wrong_length");
            }
        }

        if let Some(max) = self.constraints.max_len {
            if char_len > max {
                v.field_error(self.path.clone(), "too_long");
            }
        }

        if let Some(min) = self.constraints.min_len {
            if char_len < min {
                v.field_error(self.path.clone(), "too_short");
            }
        }
    }

    // ── Collection checks ───────────────────────────────────────────────

    fn check_collection(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        let mat = match fields.materialize(path) {
            Some(m) => m,
            None => return,
        };

        let item_count = match &mat {
            InnerValue::List(l) => l.len() as u64,
            InnerValue::Map(m) => m.len() as u64,
            InnerValue::Set(s) => s.len() as u64,
            _ => return,
        };

        if let Some(exact) = self.constraints.len {
            if item_count != exact {
                v.field_error(self.path.clone(), "wrong_length");
            }
        }

        if let Some(max) = self.constraints.max_len {
            if item_count > max {
                v.field_error(self.path.clone(), "too_long");
            }
        }

        if let Some(min) = self.constraints.min_len {
            if item_count < min {
                v.field_error(self.path.clone(), "too_short");
            }
        }

        // array_of — element type check for List only.
        if let (TypeTag::List, Some(elem_tag)) = (self.ty, self.constraints.array_of) {
            if let InnerValue::List(items) = &mat {
                self.check_array_of(items, elem_tag, v);
            }
        }
    }

    /// Verify every element in a list matches `elem_tag`.
    fn check_array_of(&self, items: &[InnerValue], elem_tag: TypeTag, v: &mut Validation) {
        for item in items {
            let matches = match elem_tag {
                TypeTag::String => matches!(item, InnerValue::Str(_)),
                TypeTag::Int => matches!(item, InnerValue::Int(_)),
                TypeTag::F64 => matches!(item, InnerValue::F64(_)),
                TypeTag::Bool => matches!(item, InnerValue::Bool(_)),
                TypeTag::Bin => matches!(item, InnerValue::Bin(_)),
                TypeTag::Dec => matches!(item, InnerValue::Dec(_)),
                TypeTag::Null => matches!(item, InnerValue::Null),
                TypeTag::List => matches!(item, InnerValue::List(_)),
                TypeTag::Map => matches!(item, InnerValue::Map(_)),
                TypeTag::Set => matches!(item, InnerValue::Set(_)),
                TypeTag::Any => true,
            };
            if !matches {
                v.field_error(self.path.clone(), "type_mismatch");
                return; // one error per rule, not per element
            }
        }
    }

    // ── one_of ──────────────────────────────────────────────────────────

    fn check_one_of(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        let allowed = match &self.constraints.one_of {
            Some(vals) if !vals.is_empty() => vals,
            _ => return,
        };

        // Materialise the value and convert to QueryValue for comparison.
        let actual = self.materialize_as_qv(fields, path);
        let actual = match actual {
            Some(qv) => qv,
            None => {
                v.field_error(self.path.clone(), "not_in_enum");
                return;
            }
        };

        if !allowed.contains(&actual) {
            v.field_error(self.path.clone(), "not_in_enum");
        }
    }

    /// Materialise the field value as a [`QueryValue`] for `one_of` comparison.
    fn materialize_as_qv(&self, fields: &dyn RecordFields, path: &[&str]) -> Option<QueryValue> {
        // Try scalar first (cheap, borrow-based).
        if let Some(sr) = fields.scalar(path) {
            return Some(scalar_ref_to_qv(sr));
        }
        // Fall back to materialise for containers / Dec.
        fields.materialize(path).map(inner_to_qv)
    }

    // ── Any-typed constraints ───────────────────────────────────────────

    /// For `TypeTag::Any`, we still check numeric/string/collection
    /// constraints if the runtime value happens to be of the matching kind.
    fn check_any(&self, fields: &dyn RecordFields, path: &[&str], v: &mut Validation) {
        // Try numeric checks if the value is Int or F64.
        match fields.scalar(path) {
            Some(ScalarRef::Int(_)) => self.check_int(fields, path, v),
            Some(ScalarRef::F64(_)) => self.check_f64(fields, path, v),
            Some(ScalarRef::Str(_)) => self.check_string(fields, path, v),
            _ => {}
        }
        // Collection checks if materialised as a container.
        if let Some(InnerValue::List(_) | InnerValue::Map(_) | InnerValue::Set(_)) =
            fields.materialize(path)
        {
            self.check_collection(fields, path, v);
        }
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn scalar_ref_to_qv(sr: ScalarRef<'_>) -> QueryValue {
    match sr {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(b),
        ScalarRef::Int(i) => QueryValue::Int(i),
        ScalarRef::F64(f) => QueryValue::F64(f),
        ScalarRef::Str(s) => QueryValue::Str(s.to_owned()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.to_vec()),
    }
}

fn inner_to_qv(iv: InnerValue) -> QueryValue {
    match iv {
        InnerValue::Null => QueryValue::Null,
        InnerValue::Bool(b) => QueryValue::Bool(b),
        InnerValue::Int(i) => QueryValue::Int(i),
        InnerValue::F64(f) => QueryValue::F64(f),
        InnerValue::Str(s) => QueryValue::Str(s),
        InnerValue::Bin(b) => QueryValue::Bin(b),
        InnerValue::Dec(d) => QueryValue::Dec(d),
        InnerValue::Big(b) => QueryValue::Big(b),
        InnerValue::List(l) => QueryValue::List(l.into_iter().map(inner_to_qv).collect()),
        InnerValue::Set(s) => {
            let mut ts = shamir_types::types::common::new_set();
            for item in s {
                ts.insert(inner_to_qv(item));
            }
            QueryValue::Set(ts)
        }
        InnerValue::Map(m) => {
            // InnerValue::Map keys are InternerKey — we cannot convert them
            // back to strings without an interner, so we drop to Null.
            // one_of on Map values is an edge case rarely used in practice.
            let _ = m;
            QueryValue::Null
        }
    }
}
