//! CallOp — stored procedure / callable function batch operation.

use serde::{Deserialize, Serialize};

use crate::filter::FilterValue;

fn default_repo() -> String {
    "main".to_string()
}

/// A batch operation that calls a named stored function.
///
/// # Wire format
///
/// ```text
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
