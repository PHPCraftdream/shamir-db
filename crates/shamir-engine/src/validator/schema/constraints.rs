//! Constraint descriptors for declarative schema field rules.
//!
//! [`Constraints`] holds the set of checks applied to a field value after
//! the type tag passes.  Phase A covers pure (in-process, no DB) checks;
//! Phases B/C will extend this struct with `scalar`, `foreign_key`, `unique`.

use shamir_types::types::value::QueryValue;

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

/// Pure (in-process) constraints for a field rule.
///
/// All fields are public for construction in tests and from the fluent
/// builder.  Phase B will add `scalar: Option<ScalarRef>` (escape-hatch);
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
}
