//! Foreign-key reference descriptor (Phase C2).
//!
//! [`ForeignKeyRef`] identifies the parent table and field that the child
//! field must reference.  The existence check is performed at write time by
//! [`SchemaValidator`](super::schema_validator::SchemaValidator) via
//! [`ValidatorDb::exists_in`](crate::validator::validator_db::ValidatorDb::exists_in).

/// A forward-only foreign-key reference.
///
/// `ref_table` and `ref_field` use flat (de-interned) names — the same
/// representation as the rest of the declarative schema wire format.
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignKeyRef {
    /// The parent table name (flat, same repo).
    pub ref_table: String,
    /// The field in the parent table that must contain the referenced value.
    pub ref_field: String,
}

impl ForeignKeyRef {
    /// Construct a new foreign-key reference.
    pub fn new(ref_table: impl Into<String>, ref_field: impl Into<String>) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
        }
    }
}
