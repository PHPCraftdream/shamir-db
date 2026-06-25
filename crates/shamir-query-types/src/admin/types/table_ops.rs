//! Table-level DDL operations: create / drop / rename table.

use serde::{Deserialize, Serialize};

use super::db_ops::is_false;
use super::retention::Retention;
use super::schema_ops::FieldRuleDto;

fn default_repo() -> String {
    "main".to_string()
}

/// Create a table in a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTableOp {
    pub create_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    /// When `true`, a pre-existing table with the same name is NOT an
    /// error — the operation returns `{"created": false, "existed": true}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_not_exists: bool,
    /// Optional per-table history retention applied at creation time.
    /// `None` (absent on the wire) = CurrentOnly — today's default
    /// behaviour (no history retained). See [`Retention`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention: Option<Retention>,
    /// Optional declarative schema applied at creation time.
    /// When present, the table is created with this schema already active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Vec<FieldRuleDto>>,
}

/// Drop a table.
///
/// Requires `hmac` over `b"drop_table\0<db_in_use>\0<repo>\0<table>"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropTableOp {
    pub drop_table: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
    /// When `true`, dropping a non-existent table (or a table whose
    /// parent db/repo is missing) is a silent no-op returning
    /// `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
    /// When `true`, the table's own artifacts (bound validators, schema,
    /// indexes) are cleaned up atomically before the table is removed.
    /// Does **not** bypass the reverse-FK guard (`drop_refused_fk`) —
    /// a foreign key from another table still blocks the drop.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cascade: bool,
}

/// Rename a table inside a repository.
///
/// The physical data stores (`__data__`, `__info__`, `__history__`) are
/// copied from the old name to the new one, the catalogue record is
/// re-keyed, and the in-memory table config is migrated. Bindings that
/// reference the table by id (record ids, index ids) travel with the
/// copied `__info__` store; bindings keyed by *name* (declarative schema
/// validator, FK references from other tables) are guarded up-front and
/// refuse the rename with a typed error code instead of leaving dangling
/// references.
///
/// ```text
/// { "rename_table": "old_name", "to": "new_name", "repo": "main" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameTableOp {
    pub rename_table: String,
    pub to: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}
