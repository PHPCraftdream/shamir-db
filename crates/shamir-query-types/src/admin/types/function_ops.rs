//! Stored-function DDL operations: create / drop / rename / folder.

use serde::{Deserialize, Serialize};

use super::db_ops::is_false;

/// Create (or replace) a stored function from Rust source or pre-compiled WASM.
///
/// Exactly one of `source` or `wasm` must be provided. `wasm` is the raw
/// binary bytes (base64-encoded on the wire).
///
/// `visibility`/`security`/`secret_grants` thread the in-process
/// `CreateFunctionOptions` fields onto the wire (task #554). Absent/empty
/// values preserve the historical defaults (`Private` / `Invoker` / no
/// grants). `security: "definer"` and non-empty `secret_grants` each
/// require a matching `hmac` tag (conditional — see `check_destructive_hmacs`).
///
/// ```text
/// { "create_function": "my_fn", "source": "pub fn shamir_call …", "replace": false }
/// { "create_function": "my_fn", "wasm": "<base64>", "replace": true }
/// { "create_function": "my_fn", "wasm": "<base64>", "security": "definer", "hmac": "<hex>" }
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
    /// `"public"` or `"private"` (parsed via `Visibility::from_str`,
    /// `shamir-wasm-host/src/meta.rs`). Absent/None → `Visibility::Private`
    /// (unchanged default). No extra gate — Private is already the
    /// default, and setting Public on your own newly-created resource is
    /// harmless (same as the existing chmod-to-Public path, which needs
    /// only ordinary owner+Manage rights already implied by CREATE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    /// `"invoker"` or `"definer"` (parsed via `Security::from_str`).
    /// Absent/None → `Security::Invoker` (unchanged default). Setting
    /// `"definer"` requires an `hmac` tag — see `hmac` below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    /// Non-empty requires BOTH `Action::Manage` on `ResourcePath::Root`
    /// AND an `hmac` tag — see `hmac` below.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_grants: Vec<String>,
    /// Hex-encoded HMAC-SHA256 tag, required IFF `security == Some("definer")`
    /// or `secret_grants` is non-empty (conditional — NOT required for
    /// every `CreateFunctionOp`, unlike `chmod`/`drop_db`/etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
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
