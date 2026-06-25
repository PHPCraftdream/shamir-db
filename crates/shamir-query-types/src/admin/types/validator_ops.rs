//! Validator DDL operations: create / drop / rename / bind / unbind / list.

use serde::{Deserialize, Serialize};

use super::db_ops::is_false;

fn default_repo() -> String {
    "main".to_string()
}

/// Create (or replace) a validator from Rust source or pre-compiled WASM.
///
/// ```text
/// { "create_validator": "v_age", "source": "pub fn shamir_call …", "replace": false }
/// { "create_validator": "v_age", "wasm": "<base64>", "replace": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateValidatorOp {
    pub create_validator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

/// Drop a validator by name.
///
/// ```text
/// { "drop_validator": "v_age" }
/// { "drop_validator": "v_age", "if_exists": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropValidatorOp {
    pub drop_validator: String,
    /// When `true`, dropping a non-existent validator is a silent no-op
    /// returning `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
}

/// Rename a validator.
///
/// ```text
/// { "rename_validator": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameValidatorOp {
    pub rename_validator: String,
    pub to: String,
}

/// Bind a validator to a table on specified write operations.
///
/// ```text
/// {
///   "bind_validator": "v_age",
///   "table": { "db": "testdb", "repo": "main", "table": "users" },
///   "ops": ["insert", "update"],
///   "priority": 1500
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindValidatorOp {
    pub bind_validator: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub table: String,
    pub ops: Vec<crate::WriteOp>,
    pub priority: u16,
}

/// Unbind a validator from a table.
///
/// ```text
/// { "unbind_validator": "v_age", "table": { "db": "testdb", "repo": "main", "table": "users" } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnbindValidatorOp {
    pub unbind_validator: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
    pub table: String,
}

/// List validator bindings for a table.
///
/// ```text
/// { "list_validators": "users", "db": "testdb", "repo": "main" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListValidatorsOp {
    pub list_validators: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}
