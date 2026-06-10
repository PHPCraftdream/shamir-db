//! Validator DDL operations: create / drop / rename / bind / unbind / list.

use serde::{Deserialize, Serialize};

fn default_repo() -> String {
    "main".to_string()
}

/// Create (or replace) a validator from Rust source or pre-compiled WASM.
///
/// ```json
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
/// ```json
/// { "drop_validator": "v_age" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropValidatorOp {
    pub drop_validator: String,
}

/// Rename a validator.
///
/// ```json
/// { "rename_validator": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameValidatorOp {
    pub rename_validator: String,
    pub to: String,
}

/// Bind a validator to a table on specified write operations.
///
/// ```json
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
/// ```json
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
/// ```json
/// { "list_validators": "users", "db": "testdb", "repo": "main" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListValidatorsOp {
    pub list_validators: String,
    pub db: String,
    #[serde(default = "default_repo")]
    pub repo: String,
}
