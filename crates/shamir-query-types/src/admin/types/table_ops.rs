//! Table-level DDL operations: create / drop table.

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
}
