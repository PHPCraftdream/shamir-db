//! Referential action for a foreign-key constraint (Phase D.0).
//!
//! [`FkAction`] is the wire/DTO representation of the `on_delete` rule.  It is
//! carried by [`ForeignKeyDto`](super::ForeignKeyDto) and round-trips through
//! msgpack/JSON as snake_case strings (`"no_action"` / `"restrict"` /
//! `"cascade"` / `"set_null"`).
//!
//! ## Default split (important)
//!
//! * **`FkAction::default() == NoAction`** — the *serde/wire* default.  This
//!   ensures EXISTING persisted schemas (stored without an `on_delete` field)
//!   deserialize to `NoAction` and do NOT change delete behavior on reload.
//!   This is a hard backward-compat requirement.
//!
//! * The **builder** default for a *new* foreign key is `Restrict`
//!   (safe-by-default); the builder sets `Restrict` EXPLICITLY and must NOT
//!   rely on `FkAction::default()`.

use serde::{Deserialize, Serialize};

/// Referential action applied when a parent row is deleted.
///
/// Wire form is snake_case (e.g. `SetNull` → `"set_null"`).  The serde default
/// is [`FkAction::NoAction`]; see the module docs for the default split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FkAction {
    /// SQL `NO ACTION` — defer the referential check (no cascading work).
    /// This is the serde/wire default so legacy schemas round-trip unchanged.
    #[default]
    NoAction,
    /// SQL `RESTRICT` — reject the parent delete if children exist.
    /// This is the builder default for newly-declared foreign keys.
    Restrict,
    /// SQL `CASCADE` — delete the child rows when the parent is deleted.
    Cascade,
    /// SQL `SET NULL` — null the child's referencing column on parent delete.
    SetNull,
}

impl FkAction {
    /// Returns `true` when this is the serde-default [`FkAction::NoAction`].
    ///
    /// Used by `#[serde(skip_serializing_if = "FkAction::is_no_action")]` so
    /// that the wire bytes of a legacy `NoAction` foreign key are unchanged
    /// (the `on_delete` field is omitted entirely).
    #[inline]
    pub fn is_no_action(&self) -> bool {
        matches!(self, FkAction::NoAction)
    }
}
