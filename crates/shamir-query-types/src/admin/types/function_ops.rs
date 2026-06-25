//! Stored-function DDL operations: create / drop / rename / folder.

use serde::{Deserialize, Serialize};

use super::db_ops::is_false;

/// Create (or replace) a stored function from Rust source or pre-compiled WASM.
///
/// Exactly one of `source` or `wasm` must be provided. `wasm` is the raw
/// binary bytes (base64-encoded on the wire).
///
/// ```text
/// { "create_function": "my_fn", "source": "pub fn shamir_call …", "replace": false }
/// { "create_function": "my_fn", "wasm": "<base64>", "replace": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFunctionOp {
    pub create_function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    #[serde(default)]
    pub replace: bool,
}

/// Drop a stored function by name.
///
/// ```text
/// { "drop_function": "my_fn" }
/// { "drop_function": "my_fn", "if_exists": true }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropFunctionOp {
    pub drop_function: String,
    /// When `true`, dropping a non-existent function is a silent no-op
    /// returning `{"existed": false}` instead of an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub if_exists: bool,
}

/// Rename a stored function.
///
/// ```text
/// { "rename_function": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameFunctionOp {
    pub rename_function: String,
    pub to: String,
}

/// Create a function folder by path segments.
///
/// ```text
/// { "create_function_folder": ["reports", "daily"] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFunctionFolderOp {
    pub create_function_folder: Vec<String>,
}

/// Rename a function folder (and all nested descendants) by path segments.
///
/// Both `rename_function_folder` (source path) and `to` (destination path) are
/// segment vectors. The rename rekeys the folder record at `from` and every
/// descendant whose path is prefixed by `from + "/"`, preserving `ResourceMeta`.
///
/// ```text
/// { "rename_function_folder": ["a", "b"], "to": ["a", "c"] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameFunctionFolderOp {
    pub rename_function_folder: Vec<String>,
    pub to: Vec<String>,
}
