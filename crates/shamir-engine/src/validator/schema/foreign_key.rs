//! Foreign-key reference descriptor (Phase C2).
//!
//! [`ForeignKeyRef`] identifies the parent table and field that the child
//! field must reference.  The existence check is performed at write time by
//! [`SchemaValidator`](super::schema_validator::SchemaValidator) via
//! [`ValidatorDb::exists_in`](crate::validator::validator_db::ValidatorDb::exists_in).
//!
//! Phase D adds [`on_delete`](ForeignKeyRef::on_delete) — the referential
//! action applied when the parent row is deleted.

use shamir_query_types::admin::FkAction;

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
    /// Referential action on parent delete (Phase D).
    ///
    /// Default is [`FkAction::NoAction`] — legacy schemas constructed via
    /// `ForeignKeyRef::new` (which pre-dates Phase D) get `NoAction` so
    /// existing delete behavior is unchanged.
    pub on_delete: FkAction,
}

impl ForeignKeyRef {
    /// Construct a new foreign-key reference with `on_delete = NoAction`.
    ///
    /// This preserves backward compatibility for callers that pre-date
    /// Phase D. Use [`with_on_delete`](Self::with_on_delete) to specify
    /// a different action.
    pub fn new(ref_table: impl Into<String>, ref_field: impl Into<String>) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
            on_delete: FkAction::NoAction,
        }
    }

    /// Construct a foreign-key reference with an explicit `on_delete` action.
    pub fn with_on_delete(
        ref_table: impl Into<String>,
        ref_field: impl Into<String>,
        on_delete: FkAction,
    ) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
            on_delete,
        }
    }
}
