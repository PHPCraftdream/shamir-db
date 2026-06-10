//! Stored-function DDL operations: create / drop / rename / folder.

use serde::{Deserialize, Serialize};

/// Create (or replace) a stored function from Rust source or pre-compiled WASM.
///
/// Exactly one of `source` or `wasm` must be provided. `wasm` is the raw
/// binary bytes (base64-encoded on the wire).
///
/// ```json
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
/// ```json
/// { "drop_function": "my_fn" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropFunctionOp {
    pub drop_function: String,
}

/// Rename a stored function.
///
/// ```json
/// { "rename_function": "old_name", "to": "new_name" }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameFunctionOp {
    pub rename_function: String,
    pub to: String,
}

/// Create a function folder by path segments.
///
/// ```json
/// { "create_function_folder": ["reports", "daily"] }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFunctionFolderOp {
    pub create_function_folder: Vec<String>,
}
