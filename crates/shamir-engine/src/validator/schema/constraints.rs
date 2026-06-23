//! Constraint descriptors for declarative schema field rules.
//!
//! [`Constraints`] holds the set of checks applied to a field value after
//! the type tag passes.  Phase A covers pure (in-process, no DB) checks;
//! Phase B adds `scalar` (escape-hatch via a registered scalar),
//! `format` (email/url/uuid/date), and `compare` (cross-field).
//! Phase C will extend this struct with `foreign_key`, `unique`.

use shamir_types::types::value::QueryValue;

use super::cross_field::CrossFieldCompare;
use super::foreign_key::ForeignKeyRef;
use super::format::FormatKind;
use super::type_tag::TypeTag;

/// Numeric bound for `min` / `max` constraints.
///
/// Supports both integer and floating-point bounds so the same constraint
/// struct works for `Int`, `F64`, and `Dec` fields.
#[derive(Debug, Clone, PartialEq)]
pub enum Num {
    /// Integer bound (for `Int` fields).
    Int(i64),
    /// Floating-point bound (for `F64` fields).
    F64(f64),
}

/// Constraints for a field rule.
///
/// All fields are public for construction in tests and from the fluent
/// builder.  Phase B adds the `scalar`, `format`, and `compare` fields;
/// Phase C will add `foreign_key` and `unique`.
#[derive(Debug, Clone, Default)]
pub struct Constraints {
    /// The field must be present in the record.
    pub required: bool,
    /// The field may be `Null`.  If `false` and the field is present as
    /// `Null`, validation fails with `null_not_allowed`.
    pub nullable: bool,
    /// Minimum numeric value (inclusive).  Applies to `Int` / `F64` fields.
    pub min: Option<Num>,
    /// Maximum numeric value (inclusive).  Applies to `Int` / `F64` fields.
    pub max: Option<Num>,
    /// Exact length for strings (`String` tag) or exact item count for
    /// collections (`List` / `Map` / `Set` tags).
    pub len: Option<u64>,
    /// Maximum length for strings.  Separate from `len` (which is exact).
    pub max_len: Option<u64>,
    /// Minimum length for strings.
    pub min_len: Option<u64>,
    /// `Int` field must be >= 0 (the "u64 intent" pattern: `Int + unsigned`).
    pub unsigned: bool,
    /// Enumerated allowed values.  A single-element vec is a `const` check.
    pub one_of: Option<Vec<QueryValue>>,
    /// Element type for `List` fields (`array_of`).  Only meaningful when
    /// the field's [`TypeTag`] is `List`.
    pub array_of: Option<TypeTag>,
    /// Phase B — scalar-bridge: validate the field by calling the named
    /// registered scalar (built-in funclib or user scalar) as a predicate.
    /// The scalar receives the materialised field value as its single
    /// argument and must return `Bool`.  When the resolver is unavailable
    /// (`ValidatorCtx::scalars() == None`) the rule is silently skipped.
    pub scalar: Option<String>,
    /// Phase B — named format check (`email` / `url` / `uuid` / `date`).
    /// Implemented as a thin in-process predicate; reuses funclib regex
    /// patterns where they exist (email/url/uuid) to avoid duplication.
    pub format: Option<FormatKind>,
    /// Phase B — cross-field comparison against another path in the same
    /// record (e.g. `start <= end`).
    pub compare: Option<CrossFieldCompare>,
    /// Phase C2 — forward-only foreign-key reference.  The field value must
    /// exist in `ref_table.ref_field`.  Checked only when `ctx.db() == Some`
    /// (tx-mode write path); silently skipped under autocommit (no resolver).
    /// NULL values bypass the FK check (standard SQL semantics).
    pub foreign_key: Option<ForeignKeyRef>,
    /// Phase C3 — unique constraint.  The field value must not duplicate any
    /// existing committed row (or staged write within the same tx).
    /// Checked only when `ctx.db() == Some` (tx-mode write path); silently
    /// skipped under autocommit (no resolver wired — same precedent as FK).
    /// NULL values bypass the unique check (standard SQL semantics: NULL is
    /// never equal to anything, including another NULL).
    pub unique: bool,
}
