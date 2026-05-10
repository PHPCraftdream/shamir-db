//! System function call ($fn).

use serde::{Deserialize, Serialize};

use super::FilterValue;

/// System function call ($fn).
///
/// Supports both simple (no args) and complex (with args) forms.
///
/// # Examples
///
/// ```json
/// // Simple (no args)
/// { "$fn": "NOW" }
/// { "$fn": "UUID" }
///
/// // With args
/// { "$fn": { "name": "COALESCE", "args": [null, "default"] } }
/// { "$fn": { "name": "SUBSTRING", "args": [{ "$ref": "name" }, 0, 10] } }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FnCall {
    /// Simple form: just function name (no arguments)
    Simple(String),
    /// Complex form: name + arguments
    Complex {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<FilterValue>,
    },
}

impl FnCall {
    /// Create a simple function call (no args)
    pub fn simple(name: impl Into<String>) -> Self {
        FnCall::Simple(name.into())
    }

    /// Create a complex function call with args
    pub fn complex(name: impl Into<String>, args: Vec<FilterValue>) -> Self {
        FnCall::Complex {
            name: name.into(),
            args,
        }
    }

    /// Get the function name
    pub fn name(&self) -> &str {
        match self {
            FnCall::Simple(name) => name,
            FnCall::Complex { name, .. } => name,
        }
    }

    /// Get the arguments (empty for simple form)
    pub fn args(&self) -> &[FilterValue] {
        match self {
            FnCall::Simple(_) => &[],
            FnCall::Complex { args, .. } => args,
        }
    }
}
