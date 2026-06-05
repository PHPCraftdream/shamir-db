//! CallOp — stored procedure / callable function batch operation.
//!
//! A `CallOp` invokes a named WASM function as a top-level batch entry:
//! `{ "call": "fn_name", "params": [1, 2, "value"], "repo": "main" }`.
//!
//! The function runs server-side via `invoke_function_in_db_as` and returns
//! a `QueryResult` with the answer in `value` (object/array/scalar/null).

use serde::{Deserialize, Serialize};

use crate::filter::FilterValue;

fn default_repo() -> String {
    "main".to_string()
}

/// A batch operation that calls a named stored function.
///
/// # Wire format
///
/// ```json
/// {
///     "call": "my_procedure",
///     "params": [1, "hello", true],
///     "repo": "main"
/// }
/// ```
///
/// `params` are positional [`FilterValue`]s — they may be literals or
/// `$query` references to other batch results (Phase 2 dependency-graph).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallOp {
    /// Name of the function to invoke.
    pub call: String,

    /// Positional parameters passed to the function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<FilterValue>,

    /// Repository context for the function invocation.
    #[serde(default = "default_repo")]
    pub repo: String,
}
