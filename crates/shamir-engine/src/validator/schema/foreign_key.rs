//! Foreign-key reference descriptor (Phase C2).
//!
//! [`ForeignKeyRef`] identifies the parent table and field that the child
//! field must reference.  The existence check is performed at write time by
//! [`SchemaValidator`](super::schema_validator::SchemaValidator) via
//! [`ValidatorDb::exists_in`](crate::validator::validator_db::ValidatorDb::exists_in).
//!
//! Phase D adds [`on_delete`](ForeignKeyRef::on_delete) — the referential
//! action applied when the parent row is deleted.
//!
//! Phase ②.2a adds [`on_update`](ForeignKeyRef::on_update) — the surface
//! mirror of `on_delete` (wire + DTO + builders). No enforcement yet; that
//! lands in ②.2b.

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
    /// Referential action on parent update (Phase ②.2a — surface only).
    ///
    /// Default is [`FkAction::NoAction`]; mirrors `on_delete` for backward
    /// compat (legacy schemas without `on_update` deserialize to `NoAction`).
    pub on_update: FkAction,
}

impl ForeignKeyRef {
    /// Construct a new foreign-key reference with both actions set to
    /// `NoAction`.
    ///
    /// This preserves backward compatibility for callers that pre-date
    /// Phase D / ②.2a. Use [`with_on_delete`](Self::with_on_delete) /
    /// [`with_on_update`](Self::with_on_update) /
    /// [`with_actions`](Self::with_actions) to specify non-default actions.
    pub fn new(ref_table: impl Into<String>, ref_field: impl Into<String>) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
            on_delete: FkAction::NoAction,
            on_update: FkAction::NoAction,
        }
    }

    /// Construct a foreign-key reference with an explicit `on_delete` action
    /// and `on_update = NoAction`.
    pub fn with_on_delete(
        ref_table: impl Into<String>,
        ref_field: impl Into<String>,
        on_delete: FkAction,
    ) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
            on_delete,
            on_update: FkAction::NoAction,
        }
    }

    /// Construct a foreign-key reference with an explicit `on_update` action
    /// and `on_delete = NoAction`.
    ///
    /// Phase ②.2a — surface mirror of [`with_on_delete`](Self::with_on_delete).
    pub fn with_on_update(
        ref_table: impl Into<String>,
        ref_field: impl Into<String>,
        on_update: FkAction,
    ) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
            on_delete: FkAction::NoAction,
            on_update,
        }
    }

    /// Construct a foreign-key reference with both actions set explicitly.
    ///
    /// Phase ②.2a — combined constructor for FKs that need both `on_delete`
    /// and `on_update` actions.
    pub fn with_actions(
        ref_table: impl Into<String>,
        ref_field: impl Into<String>,
        on_delete: FkAction,
        on_update: FkAction,
    ) -> Self {
        Self {
            ref_table: ref_table.into(),
            ref_field: ref_field.into(),
            on_delete,
            on_update,
        }
    }
}
